use std::{collections::HashSet, sync::Arc, time::Duration};

use chrono::Utc;
use jit_artifact::{ArtifactStore, StoredArtifact, ToolContract, ToolTestCase, ValidationSummary};
use jit_protocol::{IoFormat, JobStage};
use jit_storage::{ClaimedSynthesisJob, PublishedArtifact, Registry, StorageError};
use rig_core::{
    OneOrMany,
    agent::{AgentRun, AgentRunStep, InvalidToolCallHookAction, ModelTurnOutcome, PendingToolCall},
    completion::{Message, ToolDefinition},
    message::{ToolChoice, ToolResultContent, UserContent},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::time::{interval, sleep};
use tracing::{error, info, warn};

use crate::{
    runner::{DockerRunner, RunnerError},
    synthesizer::{
        AGENT_SYSTEM_PROMPT, FixtureContext, ModelRequest, SynthesisDraft, SynthesisError,
        Synthesizer, TestOrigin, TestVerdictKind, TestVerificationRequest,
        ValidationFailureSnapshot, validate_reason, validate_source,
    },
};

const JOB_LEASE_SECONDS: i64 = 300;
const LEASE_RENEW_INTERVAL: Duration = Duration::from_secs(30);
const MAX_JOB_ATTEMPTS: u32 = 3;
const MAX_AGENT_TURNS: usize = 9;
const MAX_SOURCE_REVISIONS: u32 = 4;
const MAX_GENERATED_TEST_CORRECTIONS: u32 = 3;
const MAX_PROBES: u32 = 3;
const AGENT_ENGINE: &str = "rig-agent-run";
const AGENT_ENGINE_VERSION: &str = "rig-core-0.40.0+jitforge-agent-v1";

pub struct JobProcessor {
    registry: Registry,
    store: ArtifactStore,
    runner: Arc<DockerRunner>,
    synthesizer: Arc<dyn Synthesizer>,
    worker_id: String,
}

impl JobProcessor {
    pub fn new(
        registry: Registry,
        store: ArtifactStore,
        runner: Arc<DockerRunner>,
        synthesizer: Arc<dyn Synthesizer>,
        worker_id: String,
    ) -> Self {
        Self {
            registry,
            store,
            runner,
            synthesizer,
            worker_id,
        }
    }

    pub async fn run(self) {
        loop {
            match self
                .registry
                .claim_synthesis_job(&self.worker_id, JOB_LEASE_SECONDS)
                .await
            {
                Ok(Some(job)) => self.handle_job(job).await,
                Ok(None) => sleep(Duration::from_millis(500)).await,
                Err(error) => {
                    error!(%error, "failed to claim synthesis job");
                    sleep(Duration::from_secs(2)).await;
                }
            }
        }
    }

    async fn handle_job(&self, job: ClaimedSynthesisJob) {
        info!(
            job_id = %job.job_id,
            tool = %job.tool,
            revision = job.revision,
            attempt = job.attempts,
            "processing synthesis job"
        );
        let result = if job.attempts > MAX_JOB_ATTEMPTS {
            Err(JobError::AttemptsExhausted)
        } else {
            let process = self.process_job(&job);
            tokio::pin!(process);
            let mut renewals = interval(LEASE_RENEW_INTERVAL);
            renewals.tick().await;
            loop {
                tokio::select! {
                    result = &mut process => break result,
                    _ = renewals.tick() => {
                        if let Err(error) = self.registry
                            .renew_job_lease(job.job_id, &self.worker_id, JOB_LEASE_SECONDS)
                            .await
                        {
                            break Err(JobError::Storage(error));
                        }
                    }
                }
            }
        };
        if let Err(error) = result {
            warn!(job_id = %job.job_id, %error, "synthesis job rejected");
            if let Err(reject_error) = self
                .registry
                .reject_job(
                    job.job_id,
                    &self.worker_id,
                    error.code(),
                    &error.to_string(),
                    error.details().as_ref(),
                )
                .await
                && !matches!(reject_error, StorageError::JobLeaseLost)
            {
                error!(job_id = %job.job_id, %reject_error, "failed to reject synthesis job");
            }
        }
    }

    async fn process_job(&self, job: &ClaimedSynthesisJob) -> Result<(), JobError> {
        let (mut run, mut workspace, resumed) = load_or_create_run(job)?;
        self.save_checkpoint(job, &run, &workspace).await?;
        self.event(
            job,
            if resumed {
                "run_resumed"
            } else {
                "run_started"
            },
            json!({
                "engine": AGENT_ENGINE,
                "engine_version": AGENT_ENGINE_VERSION,
                "attempt": job.attempts
            }),
        )
        .await?;

        let result = self.drive_agent(job, &mut run, &mut workspace).await;
        let published = result.as_ref().ok().cloned();
        if let Err(error) = self
            .cleanup_candidates(job.job_id, &workspace, published.as_deref())
            .await
        {
            warn!(job_id = %job.job_id, %error, "failed to clean synthesis candidates");
        }
        result.map(|_| ())
    }

    async fn drive_agent(
        &self,
        job: &ClaimedSynthesisJob,
        run: &mut AgentRun,
        workspace: &mut AgentWorkspace,
    ) -> Result<String, JobError> {
        loop {
            self.set_phase_stage(job, workspace).await?;
            self.save_checkpoint(job, run, workspace).await?;
            match run.next_step().map_err(agent_runtime_error)? {
                AgentRunStep::CallModel {
                    prompt,
                    history,
                    turn,
                } => {
                    let tools = available_tools(workspace);
                    self.event(
                        job,
                        "model_request",
                        json!({
                            "turn": turn,
                            "prompt": &prompt,
                            "history": &history,
                            "tools": tools.iter().map(|tool| &tool.name).collect::<Vec<_>>()
                        }),
                    )
                    .await?;
                    let turn_result = self
                        .synthesizer
                        .complete(ModelRequest {
                            system: AGENT_SYSTEM_PROMPT.to_owned(),
                            prompt,
                            history,
                            tools,
                            turn,
                            fixture: fixture_context(job, workspace),
                        })
                        .await?;
                    self.event(
                        job,
                        "model_response",
                        json!({
                            "turn": turn,
                            "choice": &turn_result.choice,
                            "usage": turn_result.usage
                        }),
                    )
                    .await?;
                    let mut outcome = run
                        .model_response(turn_result)
                        .map_err(agent_runtime_error)?;
                    loop {
                        match outcome {
                            ModelTurnOutcome::Continue { .. } | ModelTurnOutcome::TurnRetried => {
                                break;
                            }
                            ModelTurnOutcome::NeedsResolution(ref invalid) => {
                                let unknown = invalid.tool_name.clone();
                                self.event(job, "invalid_tool_call", json!({"tool": unknown}))
                                    .await?;
                                outcome = run
                                    .resolve_invalid_tool_call(
                                        InvalidToolCallHookAction::retry(format!(
                                            "Tool {unknown:?} is not available in the current phase. Call exactly one advertised tool."
                                        )),
                                    )
                                    .map_err(agent_runtime_error)?;
                            }
                        }
                    }
                }
                AgentRunStep::CallTools { calls } => {
                    self.save_checkpoint(job, run, workspace).await?;
                    if let Some(results) = rejected_tool_batch_results(&calls) {
                        run.tool_results(results).map_err(agent_runtime_error)?;
                        self.event(job, "tool_batch_rejected", json!({"count": calls.len()}))
                            .await?;
                        continue;
                    }
                    let call = &calls[0];
                    self.event(
                        job,
                        "tool_call",
                        json!({
                            "name": call.tool_call.function.name,
                            "arguments": call.tool_call.function.arguments
                        }),
                    )
                    .await?;
                    match self.execute_tool(job, workspace, call).await? {
                        ToolExecution::Continue(result) => {
                            self.event(job, "tool_result", result.clone()).await?;
                            run.tool_results(vec![tool_result(call, result)])
                                .map_err(agent_runtime_error)?;
                        }
                        ToolExecution::Published(digest) => {
                            return Ok(digest);
                        }
                    }
                }
                AgentRunStep::Done(response) => {
                    return Err(JobError::AgentProtocol(format!(
                        "model ended without publishing a tool: {}",
                        truncate_text(&response.output)
                    )));
                }
            }
        }
    }

    async fn execute_tool(
        &self,
        job: &ClaimedSynthesisJob,
        workspace: &mut AgentWorkspace,
        call: &PendingToolCall,
    ) -> Result<ToolExecution, JobError> {
        let result = self.execute_tool_inner(job, workspace, call).await;
        match result {
            Err(JobError::Synthesis(error)) => Ok(ToolExecution::Continue(json!({
                "ok": false,
                "error": error.to_string()
            }))),
            Err(JobError::Json(error)) => Ok(ToolExecution::Continue(json!({
                "ok": false,
                "error": format!("invalid tool arguments: {error}")
            }))),
            other => other,
        }
    }

    async fn execute_tool_inner(
        &self,
        job: &ClaimedSynthesisJob,
        workspace: &mut AgentWorkspace,
        call: &PendingToolCall,
    ) -> Result<ToolExecution, JobError> {
        let name = call.tool_call.function.name.as_str();
        let arguments = call.tool_call.function.arguments.clone();
        match name {
            "submit_contract" => {
                let args: SubmitContractArgs = parse_args(arguments)?;
                validate_reason(&args.reason)?;
                let draft = validate_contract(args, job)?;
                self.registry
                    .save_contract(
                        job.job_id,
                        &self.worker_id,
                        &serde_json::to_value(&draft.contract)?,
                        &draft.contract.assumptions,
                    )
                    .await?;
                workspace.draft = Some(draft);
                workspace.latest_failure = None;
                Ok(ToolExecution::Continue(json!({
                    "ok": true,
                    "message": "contract accepted; write the initial source"
                })))
            }
            "write_source" => {
                let args: WriteSourceArgs = parse_args(arguments)?;
                validate_reason(&args.reason)?;
                validate_source(&args.source)?;
                if workspace.draft.is_none() {
                    return Err(invalid_action("submit a contract before source"));
                }
                if workspace.source.is_some() {
                    return Err(invalid_action(
                        "write_source is only allowed for the initial source; use edit_source",
                    ));
                }
                if workspace.metrics.source_revisions >= MAX_SOURCE_REVISIONS {
                    return Err(JobError::AgentLimit(
                        "source revision budget exhausted".to_owned(),
                    ));
                }
                workspace.source = Some(args.source);
                workspace.metrics.source_revisions += 1;
                self.evaluate_after_mutation(job, workspace).await
            }
            "edit_source" => {
                let args: EditSourceArgs = parse_args(arguments)?;
                validate_reason(&args.reason)?;
                if args.old_text.is_empty() {
                    return Err(invalid_action("old_text must not be empty"));
                }
                if workspace.metrics.source_revisions >= MAX_SOURCE_REVISIONS {
                    return Err(JobError::AgentLimit(
                        "source revision budget exhausted".to_owned(),
                    ));
                }
                let source = workspace
                    .source
                    .as_ref()
                    .ok_or_else(|| invalid_action("write the initial source before editing"))?;
                let matches = source.match_indices(&args.old_text).count();
                if matches != 1 {
                    return Err(invalid_action(format!(
                        "old_text must match exactly once, but matched {matches} times"
                    )));
                }
                let replacement = source.replacen(&args.old_text, &args.new_text, 1);
                validate_source(&replacement)?;
                if replacement == *source {
                    return Err(invalid_action("edit_source must change the source"));
                }
                workspace.source = Some(replacement);
                workspace.metrics.source_revisions += 1;
                self.evaluate_after_mutation(job, workspace).await
            }
            "probe" => {
                let args: ProbeArgs = parse_args(arguments)?;
                validate_reason(&args.reason)?;
                if workspace.metrics.probes_run >= MAX_PROBES {
                    return Err(JobError::AgentLimit("probe budget exhausted".to_owned()));
                }
                validate_probe(&args)?;
                let digest = workspace
                    .current_artifact_digest
                    .as_deref()
                    .ok_or_else(|| invalid_action("probe requires a built candidate"))?;
                self.registry
                    .set_job_stage(job.job_id, &self.worker_id, JobStage::Validating)
                    .await?;
                workspace.metrics.probes_run += 1;
                let result = match self
                    .runner
                    .execute(
                        digest,
                        &args.args,
                        args.stdin.as_bytes(),
                        Duration::from_secs(5),
                    )
                    .await
                {
                    Ok(output) => json!({
                        "ok": true,
                        "exit_code": output.exit_code,
                        "stdout": diagnostic_bytes(&output.stdout),
                        "stderr": diagnostic_bytes(&output.stderr)
                    }),
                    Err(error) => json!({"ok": false, "error": error.to_string()}),
                };
                Ok(ToolExecution::Continue(result))
            }
            "review_generated_test" => {
                let args: ReviewGeneratedTestArgs = parse_args(arguments)?;
                validate_reason(&args.reason)?;
                if workspace.metrics.generated_test_corrections >= MAX_GENERATED_TEST_CORRECTIONS {
                    return Err(JobError::AgentLimit(
                        "generated test correction budget exhausted".to_owned(),
                    ));
                }
                let draft = workspace
                    .draft
                    .as_ref()
                    .ok_or_else(|| invalid_action("contract is missing"))?;
                let failure = workspace
                    .latest_failure
                    .as_ref()
                    .filter(|failure| {
                        failure.test_origin == TestOrigin::Generated
                            && failure.test_name == args.test_name
                    })
                    .cloned()
                    .ok_or_else(|| {
                        invalid_action("only the latest failing generated test may be reviewed")
                    })?;
                let generated_test = draft
                    .tests
                    .iter()
                    .skip(draft.user_test_count)
                    .find(|test| test.name == args.test_name)
                    .cloned()
                    .ok_or_else(|| invalid_action("generated test does not exist"))?;
                let request = TestVerificationRequest {
                    intent: job.intent.clone(),
                    input_format: job.input_format,
                    output_format: job.output_format,
                    contract: draft.contract.clone(),
                    immutable_user_tests: draft.tests[..draft.user_test_count].to_vec(),
                    generated_test,
                    current_source: workspace.source.clone().unwrap_or_default(),
                    failure,
                    review_reason: args.reason,
                };
                self.event(job, "verifier_request", serde_json::to_value(&request)?)
                    .await?;
                let verdict = self.synthesizer.verify_generated_test(request).await?;
                self.event(job, "verifier_response", serde_json::to_value(&verdict)?)
                    .await?;
                match verdict.classification {
                    TestVerdictKind::ImplementationWrong => Ok(ToolExecution::Continue(json!({
                        "ok": true,
                        "classification": "implementation_wrong",
                        "reason": verdict.reason,
                        "message": "keep the test unchanged and repair the source"
                    }))),
                    TestVerdictKind::Ambiguous => Ok(ToolExecution::Continue(json!({
                        "ok": true,
                        "classification": "ambiguous",
                        "reason": verdict.reason,
                        "message": "the test was not changed; implement conservatively or abort"
                    }))),
                    TestVerdictKind::OracleWrong => {
                        let expected_stdout = verdict.expected_stdout.ok_or_else(|| {
                            invalid_action("verifier omitted corrected expected_stdout")
                        })?;
                        let expected_exit_code = verdict.expected_exit_code.ok_or_else(|| {
                            invalid_action("verifier omitted corrected expected_exit_code")
                        })?;
                        if expected_exit_code == 0 && job.output_format == IoFormat::Json {
                            serde_json::from_str::<Value>(&expected_stdout).map_err(|error| {
                                invalid_action(format!(
                                    "verifier returned invalid expected JSON: {error}"
                                ))
                            })?;
                        }
                        let draft = workspace.draft.as_mut().expect("draft checked above");
                        let test = draft
                            .tests
                            .iter_mut()
                            .skip(draft.user_test_count)
                            .find(|test| test.name == args.test_name)
                            .expect("test checked above");
                        if test.expected_stdout == expected_stdout
                            && test.expected_exit_code == expected_exit_code
                        {
                            return Err(invalid_action(
                                "verifier oracle replacement did not change the test",
                            ));
                        }
                        test.expected_stdout = expected_stdout;
                        test.expected_exit_code = expected_exit_code;
                        workspace.metrics.generated_test_corrections += 1;
                        self.evaluate_after_mutation(job, workspace).await
                    }
                }
            }
            "abort" => {
                let args: AbortArgs = parse_args(arguments)?;
                validate_reason(&args.reason)?;
                Err(JobError::AgentAborted(args.reason))
            }
            other => Err(invalid_action(format!("unknown tool {other:?}"))),
        }
    }

    async fn evaluate_after_mutation(
        &self,
        job: &ClaimedSynthesisJob,
        workspace: &mut AgentWorkspace,
    ) -> Result<ToolExecution, JobError> {
        let draft = workspace
            .draft
            .as_ref()
            .ok_or_else(|| invalid_action("contract is missing"))?;
        let source = workspace
            .source
            .as_deref()
            .ok_or_else(|| invalid_action("source is missing"))?;
        self.registry
            .set_job_stage(job.job_id, &self.worker_id, JobStage::Building)
            .await?;
        let artifact = self.store_candidate(job, draft, source, workspace.metrics)?;
        workspace.candidate_digests.insert(artifact.digest.clone());
        workspace
            .candidate_sources
            .insert(artifact.bundle.manifest.source_sha256.clone());
        workspace.current_artifact_digest = Some(artifact.digest.clone());
        self.runner.build_artifact(&artifact.digest).await?;
        self.registry
            .set_job_stage(job.job_id, &self.worker_id, JobStage::Validating)
            .await?;
        let validation = match validate_input_samples(
            &self.runner,
            &artifact.digest,
            job.output_format,
            &job.input_samples,
        )
        .await
        {
            Ok(()) => {
                validate_tests(
                    &self.runner,
                    &artifact.digest,
                    job.output_format,
                    &draft.tests,
                    draft.user_test_count,
                )
                .await
            }
            Err(failure) => Err(failure),
        };
        match validation {
            Ok(()) => {
                workspace.latest_failure = None;
                self.publish(job, &artifact).await?;
                info!(job_id = %job.job_id, digest = %artifact.digest, "tool version published");
                Ok(ToolExecution::Published(artifact.digest))
            }
            Err(failure) => {
                let result = json!({
                    "ok": false,
                    "validation_failed": &failure
                });
                workspace.latest_failure = Some(failure);
                Ok(ToolExecution::Continue(result))
            }
        }
    }

    fn store_candidate(
        &self,
        job: &ClaimedSynthesisJob,
        draft: &SynthesisDraft,
        source: &str,
        metrics: AgentMetrics,
    ) -> Result<StoredArtifact, JobError> {
        let tests_total = draft.tests.len();
        Ok(self.store.put(
            job.input_format,
            job.output_format,
            draft.contract.clone(),
            source.to_owned(),
            draft.tests.clone(),
            ValidationSummary {
                tests_total,
                tests_passed: tests_total,
                input_samples_total: job.input_samples.len(),
                input_samples_passed: job.input_samples.len(),
                repair_rounds: metrics.source_revisions.saturating_sub(1),
                agent_turns: metrics.turns,
                generated_test_corrections: metrics.generated_test_corrections,
                probes_run: metrics.probes_run,
                validated_at: Utc::now().to_rfc3339(),
            },
        )?)
    }

    async fn publish(
        &self,
        job: &ClaimedSynthesisJob,
        artifact: &StoredArtifact,
    ) -> Result<(), JobError> {
        self.registry
            .publish_job(
                job.job_id,
                &self.worker_id,
                &PublishedArtifact {
                    digest: artifact.digest.clone(),
                    relative_path: artifact.relative_path.clone(),
                    size_bytes: i64::try_from(artifact.size_bytes).unwrap_or(i64::MAX),
                    manifest: serde_json::to_value(&artifact.bundle.manifest)?,
                    contract: serde_json::to_value(&artifact.bundle.contract)?,
                    assumptions: serde_json::to_value(&artifact.bundle.contract.assumptions)?,
                    validation_summary: serde_json::to_value(&artifact.bundle.validation)?,
                },
            )
            .await?;
        Ok(())
    }

    async fn set_phase_stage(
        &self,
        job: &ClaimedSynthesisJob,
        workspace: &AgentWorkspace,
    ) -> Result<(), JobError> {
        let stage = if workspace.draft.is_none() {
            JobStage::Contract
        } else if workspace.source.is_none() {
            JobStage::Synthesizing
        } else {
            JobStage::Repairing
        };
        self.registry
            .set_job_stage(job.job_id, &self.worker_id, stage)
            .await?;
        Ok(())
    }

    async fn save_checkpoint(
        &self,
        job: &ClaimedSynthesisJob,
        run: &AgentRun,
        workspace: &AgentWorkspace,
    ) -> Result<(), JobError> {
        let checkpoint = serde_json::to_value(AgentCheckpoint { run, workspace })?;
        self.registry
            .save_agent_checkpoint(
                job.job_id,
                &self.worker_id,
                AGENT_ENGINE,
                AGENT_ENGINE_VERSION,
                &checkpoint,
            )
            .await?;
        Ok(())
    }

    async fn event(
        &self,
        job: &ClaimedSynthesisJob,
        kind: &str,
        payload: Value,
    ) -> Result<(), JobError> {
        self.registry
            .append_agent_event(job.job_id, &self.worker_id, kind, &payload)
            .await?;
        Ok(())
    }

    async fn cleanup_candidates(
        &self,
        job_id: uuid::Uuid,
        workspace: &AgentWorkspace,
        published: Option<&str>,
    ) -> Result<(), JobError> {
        let (referenced_digests, referenced_sources) = self
            .registry
            .referenced_artifacts_excluding_job(job_id)
            .await?;
        for digest in &workspace.candidate_digests {
            if Some(digest.as_str()) != published && !referenced_digests.contains(digest) {
                self.store.remove(digest)?;
            }
        }
        for source in &workspace.candidate_sources {
            if !referenced_sources.contains(source) {
                self.runner.remove_source_image(source).await?;
            }
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct AgentCheckpoint<'a> {
    run: &'a AgentRun,
    workspace: &'a AgentWorkspace,
}

#[derive(Deserialize)]
struct OwnedAgentCheckpoint {
    run: AgentRun,
    workspace: AgentWorkspace,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct AgentWorkspace {
    draft: Option<SynthesisDraft>,
    source: Option<String>,
    current_artifact_digest: Option<String>,
    latest_failure: Option<ValidationFailureSnapshot>,
    metrics: AgentMetrics,
    #[serde(default)]
    candidate_digests: HashSet<String>,
    #[serde(default)]
    candidate_sources: HashSet<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
struct AgentMetrics {
    turns: u32,
    source_revisions: u32,
    generated_test_corrections: u32,
    probes_run: u32,
}

enum ToolExecution {
    Continue(Value),
    Published(String),
}

#[derive(Deserialize)]
struct SubmitContractArgs {
    summary: String,
    #[serde(default)]
    assumptions: Vec<String>,
    #[serde(default)]
    invariants: Vec<String>,
    #[serde(default)]
    tests: Vec<ToolTestCase>,
    reason: String,
}

#[derive(Deserialize)]
struct WriteSourceArgs {
    source: String,
    reason: String,
}

#[derive(Deserialize)]
struct EditSourceArgs {
    old_text: String,
    new_text: String,
    reason: String,
}

#[derive(Deserialize)]
struct ProbeArgs {
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    stdin: String,
    reason: String,
}

#[derive(Deserialize)]
struct ReviewGeneratedTestArgs {
    test_name: String,
    reason: String,
}

#[derive(Deserialize)]
struct AbortArgs {
    reason: String,
}

fn load_or_create_run(
    job: &ClaimedSynthesisJob,
) -> Result<(AgentRun, AgentWorkspace, bool), JobError> {
    if let Some(checkpoint) = &job.agent_checkpoint {
        if checkpoint.engine != AGENT_ENGINE || checkpoint.engine_version != AGENT_ENGINE_VERSION {
            return Err(JobError::IncompatibleCheckpoint {
                engine: checkpoint.engine.clone(),
                version: checkpoint.engine_version.clone(),
            });
        }
        let checkpoint: OwnedAgentCheckpoint =
            serde_json::from_value(checkpoint.checkpoint.clone())?;
        return Ok((checkpoint.run, checkpoint.workspace, true));
    }
    let prompt = Message::user(serde_json::to_string(&json!({
        "task": "create_and_publish_unix_tool",
        "name": job.tool,
        "user_intent": job.intent,
        "input_format": job.input_format,
        "output_format": job.output_format,
        "input_samples_without_expected_output": job.input_samples,
        "immutable_user_examples": job.examples
    }))?);
    let run = AgentRun::new(prompt)
        .max_turns(MAX_AGENT_TURNS)
        .max_invalid_tool_call_retries(1)
        .with_tool_choice(ToolChoice::Required);
    Ok((run, AgentWorkspace::default(), false))
}

fn fixture_context(job: &ClaimedSynthesisJob, workspace: &AgentWorkspace) -> FixtureContext {
    FixtureContext {
        intent: job.intent.clone(),
        examples: job.examples.clone(),
        has_contract: workspace.draft.is_some(),
        current_source: workspace.source.clone(),
        latest_failure: workspace.latest_failure.clone(),
        probes_run: workspace.metrics.probes_run,
    }
}

fn available_tools(workspace: &mut AgentWorkspace) -> Vec<ToolDefinition> {
    workspace.metrics.turns = workspace.metrics.turns.saturating_add(1);
    let mut tools = vec![abort_tool()];
    if workspace.draft.is_none() {
        tools.push(submit_contract_tool());
    } else if workspace.source.is_none() {
        tools.push(write_source_tool());
    } else {
        tools.push(edit_source_tool());
        if workspace.current_artifact_digest.is_some() {
            tools.push(probe_tool());
        }
        if workspace
            .latest_failure
            .as_ref()
            .is_some_and(|failure| failure.test_origin == TestOrigin::Generated)
        {
            tools.push(review_generated_test_tool());
        }
    }
    tools
}

fn submit_contract_tool() -> ToolDefinition {
    ToolDefinition {
        name: "submit_contract".to_owned(),
        description: "Submit the contract and generated tests before writing source.".to_owned(),
        parameters: json!({
            "type": "object",
            "properties": {
                "summary": {"type": "string"},
                "assumptions": {"type": "array", "items": {"type": "string"}},
                "invariants": {"type": "array", "items": {"type": "string"}},
                "tests": {
                    "type": "array",
                    "maxItems": 12,
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {"type": "string"},
                            "args": {"type": "array", "items": {"type": "string"}},
                            "stdin": {"type": "string"},
                            "expected_stdout": {"type": "string"},
                            "expected_exit_code": {"type": "integer"}
                        },
                        "required": ["name", "args", "stdin", "expected_stdout", "expected_exit_code"],
                        "additionalProperties": false
                    }
                },
                "reason": {"type": "string"}
            },
            "required": ["summary", "assumptions", "invariants", "tests", "reason"],
            "additionalProperties": false
        }),
    }
}

fn write_source_tool() -> ToolDefinition {
    ToolDefinition {
        name: "write_source".to_owned(),
        description: "Write the complete initial Python source. This is allowed once.".to_owned(),
        parameters: object_schema(
            json!({
                "source": {"type": "string"},
                "reason": {"type": "string"}
            }),
            &["source", "reason"],
        ),
    }
}

fn edit_source_tool() -> ToolDefinition {
    ToolDefinition {
        name: "edit_source".to_owned(),
        description: "Replace one exact source fragment. old_text must occur exactly once."
            .to_owned(),
        parameters: object_schema(
            json!({
                "old_text": {"type": "string"},
                "new_text": {"type": "string"},
                "reason": {"type": "string"}
            }),
            &["old_text", "new_text", "reason"],
        ),
    }
}

fn probe_tool() -> ToolDefinition {
    ToolDefinition {
        name: "probe".to_owned(),
        description: "Run one focused input against the current candidate in gVisor.".to_owned(),
        parameters: object_schema(
            json!({
                "args": {"type": "array", "items": {"type": "string"}},
                "stdin": {"type": "string"},
                "reason": {"type": "string"}
            }),
            &["args", "stdin", "reason"],
        ),
    }
}

fn review_generated_test_tool() -> ToolDefinition {
    ToolDefinition {
        name: "review_generated_test".to_owned(),
        description: "Ask the independent verifier to classify the latest failing generated test."
            .to_owned(),
        parameters: object_schema(
            json!({
                "test_name": {"type": "string"},
                "reason": {"type": "string"}
            }),
            &["test_name", "reason"],
        ),
    }
}

fn abort_tool() -> ToolDefinition {
    ToolDefinition {
        name: "abort".to_owned(),
        description: "Stop when the requirement is contradictory or unsupported.".to_owned(),
        parameters: object_schema(json!({"reason": {"type": "string"}}), &["reason"]),
    }
}

fn object_schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn validate_contract(
    mut args: SubmitContractArgs,
    job: &ClaimedSynthesisJob,
) -> Result<SynthesisDraft, JobError> {
    if args.summary.trim().is_empty() || args.summary.len() > 4096 {
        return Err(invalid_action("contract summary must contain 1-4096 bytes"));
    }
    if args.assumptions.len() > 32
        || args.invariants.len() > 32
        || args
            .assumptions
            .iter()
            .chain(&args.invariants)
            .any(|item| item.len() > 4096)
    {
        return Err(invalid_action(
            "contract assumptions or invariants exceed limits",
        ));
    }
    args.tests.truncate(12);
    let mut tests: Vec<ToolTestCase> = job
        .examples
        .iter()
        .enumerate()
        .map(|(index, example)| ToolTestCase {
            name: format!("user-example-{}", index + 1),
            args: vec![],
            stdin: example.input.clone(),
            expected_stdout: example.output.clone(),
            expected_exit_code: 0,
        })
        .collect();
    let user_test_count = tests.len();
    for (index, mut test) in args.tests.into_iter().enumerate() {
        let name: String = test.name.trim().chars().take(96).collect();
        test.name = format!(
            "generated-{}-{}",
            index + 1,
            if name.is_empty() { "test" } else { &name }
        );
        validate_test(&test, job.output_format)?;
        tests.push(test);
    }
    if tests.is_empty() {
        return Err(invalid_action("contract must contain at least one test"));
    }
    Ok(SynthesisDraft {
        contract: ToolContract {
            summary: args.summary,
            assumptions: args.assumptions,
            invariants: args.invariants,
        },
        tests,
        user_test_count,
    })
}

fn validate_test(test: &ToolTestCase, output_format: IoFormat) -> Result<(), JobError> {
    if test.name.is_empty()
        || test.name.len() > 128
        || test.stdin.len() > 1024 * 1024
        || test.expected_stdout.len() > 1024 * 1024
        || test.args.len() > 32
        || test.args.iter().any(|arg| arg.len() > 4096)
    {
        return Err(invalid_action("generated test exceeds protocol limits"));
    }
    if output_format == IoFormat::Json && test.expected_exit_code == 0 {
        serde_json::from_str::<Value>(&test.expected_stdout)
            .map_err(|error| invalid_action(format!("generated test has invalid JSON: {error}")))?;
    }
    Ok(())
}

fn validate_probe(args: &ProbeArgs) -> Result<(), JobError> {
    if args.args.len() > 32
        || args.args.iter().any(|argument| argument.len() > 4096)
        || args.stdin.len() > 1024 * 1024
    {
        return Err(invalid_action("probe exceeds protocol limits"));
    }
    Ok(())
}

fn parse_args<T: DeserializeOwned>(value: Value) -> Result<T, JobError> {
    Ok(serde_json::from_value(value)?)
}

fn invalid_action(message: impl Into<String>) -> JobError {
    JobError::Synthesis(SynthesisError::InvalidAction(message.into()))
}

fn agent_runtime_error(error: impl std::fmt::Display) -> JobError {
    JobError::AgentRuntime(error.to_string())
}

fn tool_result(call: &PendingToolCall, value: Value) -> UserContent {
    let content = OneOrMany::one(ToolResultContent::text(value.to_string()));
    if let Some(call_id) = call.tool_call.call_id.clone() {
        UserContent::tool_result_with_call_id(call.tool_call.id.clone(), call_id, content)
    } else {
        UserContent::tool_result(call.tool_call.id.clone(), content)
    }
}

fn rejected_tool_batch_results(calls: &[PendingToolCall]) -> Option<Vec<UserContent>> {
    (calls.len() != 1).then(|| {
        calls
            .iter()
            .map(|call| {
                tool_result(
                    call,
                    json!({
                        "ok": false,
                        "error": "exactly one tool call is allowed per turn"
                    }),
                )
            })
            .collect()
    })
}

async fn validate_tests(
    runner: &DockerRunner,
    digest: &str,
    output_format: IoFormat,
    tests: &[ToolTestCase],
    user_test_count: usize,
) -> Result<(), ValidationFailureSnapshot> {
    for (index, test) in tests.iter().enumerate() {
        let test_origin = if index < user_test_count {
            TestOrigin::User
        } else {
            TestOrigin::Generated
        };
        let output = match runner
            .execute(
                digest,
                &test.args,
                test.stdin.as_bytes(),
                Duration::from_secs(5),
            )
            .await
        {
            Ok(output) => output,
            Err(error) => {
                return Err(ValidationFailureSnapshot {
                    test_name: test.name.clone(),
                    test_origin,
                    diagnostic: format!("test {:?} runner error: {error}", test.name),
                    actual_stdout: String::new(),
                    actual_stderr: String::new(),
                    actual_exit_code: None,
                });
            }
        };
        if output.exit_code != test.expected_exit_code {
            return Err(ValidationFailureSnapshot {
                test_name: test.name.clone(),
                test_origin,
                diagnostic: format!(
                    "exit code mismatch: expected {}, got {}",
                    test.expected_exit_code, output.exit_code
                ),
                actual_stdout: diagnostic_bytes(&output.stdout),
                actual_stderr: diagnostic_bytes(&output.stderr),
                actual_exit_code: Some(output.exit_code),
            });
        }
        if let Err(diagnostic) =
            validate_stdout(output_format, test, &output.stdout, &output.stderr)
        {
            return Err(ValidationFailureSnapshot {
                test_name: test.name.clone(),
                test_origin,
                diagnostic,
                actual_stdout: diagnostic_bytes(&output.stdout),
                actual_stderr: diagnostic_bytes(&output.stderr),
                actual_exit_code: Some(output.exit_code),
            });
        }
    }
    Ok(())
}

async fn validate_input_samples(
    runner: &DockerRunner,
    digest: &str,
    output_format: IoFormat,
    samples: &[String],
) -> Result<(), ValidationFailureSnapshot> {
    for (index, sample) in samples.iter().enumerate() {
        let name = format!("input-sample-{}", index + 1);
        let output = match runner
            .execute(digest, &[], sample.as_bytes(), Duration::from_secs(5))
            .await
        {
            Ok(output) => output,
            Err(error) => {
                return Err(ValidationFailureSnapshot {
                    test_name: name,
                    test_origin: TestOrigin::InputSample,
                    diagnostic: format!("input sample runner error: {error}"),
                    actual_stdout: String::new(),
                    actual_stderr: String::new(),
                    actual_exit_code: None,
                });
            }
        };
        let diagnostic = if output.exit_code != 0 {
            Some(format!(
                "input sample must execute successfully, but exited with {}",
                output.exit_code
            ))
        } else if output_format == IoFormat::Json {
            serde_json::from_slice::<Value>(&output.stdout)
                .err()
                .map(|error| format!("input sample returned invalid JSON: {error}"))
        } else {
            std::str::from_utf8(&output.stdout)
                .err()
                .map(|error| format!("input sample returned non-UTF-8 text: {error}"))
        };
        if let Some(diagnostic) = diagnostic {
            return Err(ValidationFailureSnapshot {
                test_name: name,
                test_origin: TestOrigin::InputSample,
                diagnostic,
                actual_stdout: diagnostic_bytes(&output.stdout),
                actual_stderr: diagnostic_bytes(&output.stderr),
                actual_exit_code: Some(output.exit_code),
            });
        }
    }
    Ok(())
}

fn validate_stdout(
    output_format: IoFormat,
    test: &ToolTestCase,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<(), String> {
    if output_format == IoFormat::Text || test.expected_exit_code != 0 {
        if stdout != test.expected_stdout.as_bytes() {
            return Err(format!(
                "stdout mismatch: expected {:?}, got {:?}; stderr={:?}",
                truncate_text(&test.expected_stdout),
                diagnostic_bytes(stdout),
                diagnostic_bytes(stderr)
            ));
        }
        return Ok(());
    }
    let expected: Value = serde_json::from_str(&test.expected_stdout)
        .map_err(|error| format!("test {:?} has invalid expected JSON: {error}", test.name))?;
    let actual: Value = serde_json::from_slice(stdout)
        .map_err(|error| format!("test {:?} returned invalid JSON: {error}", test.name))?;
    if !json_values_equivalent(&actual, &expected) {
        return Err(format!(
            "JSON mismatch: expected {}, got {}",
            expected, actual
        ));
    }
    Ok(())
}

fn json_values_equivalent(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Number(actual), Value::Number(expected)) => {
            normalized_json_number(actual) == normalized_json_number(expected)
        }
        (Value::Array(actual), Value::Array(expected)) => {
            actual.len() == expected.len()
                && actual
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| json_values_equivalent(actual, expected))
        }
        (Value::Object(actual), Value::Object(expected)) => {
            actual.len() == expected.len()
                && actual.iter().all(|(key, actual)| {
                    expected
                        .get(key)
                        .is_some_and(|expected| json_values_equivalent(actual, expected))
                })
        }
        _ => actual == expected,
    }
}

fn normalized_json_number(number: &serde_json::Number) -> Option<(bool, String, i64)> {
    let encoded = number.to_string();
    let (mantissa, explicit_exponent) = match encoded.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i64>().ok()?),
        None => (encoded.as_str(), 0_i64),
    };
    let negative = mantissa.starts_with('-');
    let unsigned = mantissa.strip_prefix('-').unwrap_or(mantissa);
    let fraction_digits = unsigned
        .split_once('.')
        .map_or(0, |(_, fraction)| fraction.len());
    let mut exponent = explicit_exponent.checked_sub(i64::try_from(fraction_digits).ok()?)?;
    let mut digits: String = unsigned
        .chars()
        .filter(|character| *character != '.')
        .collect();
    let first_nonzero = digits.find(|character| character != '0');
    let Some(first_nonzero) = first_nonzero else {
        return Some((false, "0".to_owned(), 0));
    };
    digits.drain(..first_nonzero);
    while digits.ends_with('0') {
        digits.pop();
        exponent = exponent.checked_add(1)?;
    }
    Some((negative, digits, exponent))
}

fn diagnostic_bytes(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(2048)]).into_owned()
}

fn truncate_text(value: &str) -> String {
    value.chars().take(2048).collect()
}

#[derive(Debug, Error)]
enum JobError {
    #[error("worker retry limit exceeded")]
    AttemptsExhausted,

    #[error("synthesis agent aborted: {0}")]
    AgentAborted(String),

    #[error("synthesis agent action limit reached: {0}")]
    AgentLimit(String),

    #[error("synthesis agent protocol failed: {0}")]
    AgentProtocol(String),

    #[error("synthesis agent runtime failed: {0}")]
    AgentRuntime(String),

    #[error("agent checkpoint uses incompatible engine {engine:?} version {version:?}")]
    IncompatibleCheckpoint { engine: String, version: String },

    #[error(transparent)]
    Synthesis(#[from] SynthesisError),

    #[error(transparent)]
    Runner(#[from] RunnerError),

    #[error(transparent)]
    Artifact(#[from] jit_artifact::ArtifactError),

    #[error(transparent)]
    Storage(#[from] StorageError),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl JobError {
    fn code(&self) -> &'static str {
        match self {
            Self::AttemptsExhausted => "attempts_exhausted",
            Self::AgentAborted(_) => "agent_aborted",
            Self::AgentLimit(_) => "agent_limit_reached",
            Self::AgentProtocol(_) => "agent_protocol_failed",
            Self::AgentRuntime(_) => "agent_runtime_failed",
            Self::IncompatibleCheckpoint { .. } => "agent_checkpoint_incompatible",
            Self::Synthesis(_) => "synthesis_failed",
            Self::Runner(_) => "runner_failed",
            Self::Artifact(_) => "artifact_failed",
            Self::Storage(_) => "storage_failed",
            Self::Json(_) => "serialization_failed",
        }
    }

    fn details(&self) -> Option<Value> {
        match self {
            Self::AgentAborted(reason)
            | Self::AgentLimit(reason)
            | Self::AgentProtocol(reason)
            | Self::AgentRuntime(reason) => Some(json!({
                "reason": truncate_text(reason),
            })),
            Self::IncompatibleCheckpoint { engine, version } => Some(json!({
                "engine": engine,
                "version": version
            })),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    use rig_core::completion::{AssistantContent, Usage};

    fn claimed_job() -> ClaimedSynthesisJob {
        ClaimedSynthesisJob {
            job_id: uuid::Uuid::now_v7(),
            tool_id: uuid::Uuid::now_v7(),
            tool: "example".to_owned(),
            revision: 1,
            intent: "example".to_owned(),
            input_format: IoFormat::Text,
            output_format: IoFormat::Text,
            examples: vec![],
            input_samples: vec![],
            attempts: 1,
            agent_checkpoint: None,
        }
    }

    fn model_turn(
        contents: OneOrMany<AssistantContent>,
        tools: &[&str],
    ) -> rig_core::agent::ModelTurn {
        let tools: BTreeSet<String> = tools.iter().map(|name| (*name).to_owned()).collect();
        rig_core::agent::ModelTurn::new(None, contents, Usage::new(), tools.clone(), tools)
    }

    #[test]
    fn exact_edit_requires_one_match() {
        let source = "alpha beta alpha";
        assert_eq!(source.match_indices("alpha").count(), 2);
        assert_eq!(source.match_indices("beta").count(), 1);
    }

    #[test]
    fn successful_json_tests_still_require_json() {
        let test = ToolTestCase {
            name: "valid input".to_owned(),
            args: vec![],
            stdin: "{}".to_owned(),
            expected_stdout: "{}".to_owned(),
            expected_exit_code: 0,
        };
        assert!(validate_stdout(IoFormat::Json, &test, b"", b"").is_err());
        assert!(validate_stdout(IoFormat::Json, &test, b"{}", b"").is_ok());
    }

    #[test]
    fn json_number_comparison_is_decimal_semantic_and_exact() {
        assert!(json_values_equivalent(
            &serde_json::json!({"value": 2}),
            &serde_json::json!({"value": 2.0})
        ));
        let first: Value = serde_json::from_str("9007199254740992").unwrap();
        let second: Value = serde_json::from_str("9007199254740993").unwrap();
        assert!(!json_values_equivalent(&first, &second));
    }

    #[test]
    fn user_examples_are_prefixed_and_immutable() {
        let mut job = claimed_job();
        job.examples = vec![jit_protocol::ToolExample {
            input: "a".to_owned(),
            output: "b".to_owned(),
        }];
        let draft = validate_contract(
            SubmitContractArgs {
                summary: "example".to_owned(),
                assumptions: vec![],
                invariants: vec![],
                tests: vec![],
                reason: "test".to_owned(),
            },
            &job,
        )
        .unwrap();
        assert_eq!(draft.user_test_count, 1);
        assert_eq!(draft.tests[0].name, "user-example-1");
    }

    #[test]
    fn synthesis_prompt_distinguishes_samples_from_paired_examples() {
        let mut job = claimed_job();
        job.input_samples = vec!["Architecture: x86_64\n".to_owned()];
        job.examples = vec![jit_protocol::ToolExample {
            input: "input".to_owned(),
            output: "output".to_owned(),
        }];
        let (mut run, _, resumed) = load_or_create_run(&job).unwrap();
        assert!(!resumed);
        let AgentRunStep::CallModel { prompt, .. } = run.next_step().unwrap() else {
            panic!("new agent run should call the model");
        };
        let encoded = serde_json::to_string(&prompt).unwrap();
        assert!(encoded.contains("input_samples_without_expected_output"));
        assert!(encoded.contains("Architecture: x86_64"));
        assert!(encoded.contains("immutable_user_examples"));
    }

    #[test]
    fn malformed_arguments_and_contract_can_be_corrected() {
        assert!(parse_args::<WriteSourceArgs>(json!({"source": "print(1)"})).is_err());

        let job = claimed_job();
        let invalid = validate_contract(
            SubmitContractArgs {
                summary: "example".to_owned(),
                assumptions: vec![],
                invariants: vec![],
                tests: vec![],
                reason: "first attempt".to_owned(),
            },
            &job,
        );
        assert!(invalid.is_err());

        let corrected = validate_contract(
            SubmitContractArgs {
                summary: "example".to_owned(),
                assumptions: vec![],
                invariants: vec![],
                tests: vec![ToolTestCase {
                    name: "basic".to_owned(),
                    args: vec![],
                    stdin: "a".to_owned(),
                    expected_stdout: "b".to_owned(),
                    expected_exit_code: 0,
                }],
                reason: "add a concrete test".to_owned(),
            },
            &job,
        )
        .unwrap();
        assert_eq!(corrected.tests.len(), 1);
    }

    #[test]
    fn multiple_tool_calls_are_all_rejected_and_answered() {
        let mut run = AgentRun::new(Message::user("test"))
            .max_turns(3)
            .with_tool_choice(ToolChoice::Required);
        assert!(matches!(
            run.next_step().unwrap(),
            AgentRunStep::CallModel { .. }
        ));
        let choice = OneOrMany::many([
            AssistantContent::tool_call("call-1", "abort", json!({"reason": "one"})),
            AssistantContent::tool_call("call-2", "abort", json!({"reason": "two"})),
        ])
        .unwrap();
        assert!(matches!(
            run.model_response(model_turn(choice, &["abort"])).unwrap(),
            ModelTurnOutcome::Continue { .. }
        ));
        let AgentRunStep::CallTools { calls } = run.next_step().unwrap() else {
            panic!("expected pending tool calls");
        };
        assert_eq!(calls.len(), 2);
        let results = rejected_tool_batch_results(&calls).unwrap();
        assert_eq!(results.len(), 2);
        run.tool_results(results).unwrap();
        assert!(matches!(
            run.next_step().unwrap(),
            AgentRunStep::CallModel { turn: 2, .. }
        ));
    }

    #[test]
    fn unknown_tool_call_gets_one_corrective_retry() {
        let mut run = AgentRun::new(Message::user("test"))
            .max_turns(3)
            .max_invalid_tool_call_retries(1)
            .with_tool_choice(ToolChoice::Required);
        assert!(matches!(
            run.next_step().unwrap(),
            AgentRunStep::CallModel { .. }
        ));
        let outcome = run
            .model_response(model_turn(
                OneOrMany::one(AssistantContent::tool_call("call-1", "unknown", json!({}))),
                &["abort"],
            ))
            .unwrap();
        assert!(matches!(outcome, ModelTurnOutcome::NeedsResolution(_)));
        assert!(matches!(
            run.resolve_invalid_tool_call(InvalidToolCallHookAction::retry(
                "Call exactly one advertised tool."
            ))
            .unwrap(),
            ModelTurnOutcome::TurnRetried
        ));
        let AgentRunStep::CallModel {
            prompt,
            history,
            turn,
        } = run.next_step().unwrap()
        else {
            panic!("expected retried model call");
        };
        assert_eq!(turn, 2);
        let request = serde_json::to_string(&(history, prompt)).unwrap();
        assert!(request.contains("Call exactly one advertised tool"));
    }

    #[test]
    fn checkpoint_replays_a_pending_model_call() {
        let job = claimed_job();
        let (mut run, workspace, resumed) = load_or_create_run(&job).unwrap();
        assert!(!resumed);
        let checkpoint = serde_json::to_value(AgentCheckpoint {
            run: &run,
            workspace: &workspace,
        })
        .unwrap();
        let AgentRunStep::CallModel {
            prompt: expected_prompt,
            history: expected_history,
            turn: expected_turn,
        } = run.next_step().unwrap()
        else {
            panic!("expected model call");
        };

        let mut resumed: OwnedAgentCheckpoint = serde_json::from_value(checkpoint).unwrap();
        let AgentRunStep::CallModel {
            prompt,
            history,
            turn,
        } = resumed.run.next_step().unwrap()
        else {
            panic!("expected replayed model call");
        };
        let prompt_text = |message: &Message| match message {
            Message::User { content } => content.iter().find_map(|item| match item {
                UserContent::Text(text) => Some(text.text.clone()),
                _ => None,
            }),
            _ => None,
        };
        assert_eq!(prompt_text(&prompt), prompt_text(&expected_prompt));
        assert_eq!(history, expected_history);
        assert_eq!(turn, expected_turn);
    }

    #[test]
    fn checkpoint_replays_a_pending_tool_call() {
        let mut run = AgentRun::new(Message::user("test"))
            .max_turns(2)
            .with_tool_choice(ToolChoice::Required);
        assert!(matches!(
            run.next_step().unwrap(),
            AgentRunStep::CallModel { .. }
        ));
        run.model_response(model_turn(
            OneOrMany::one(AssistantContent::tool_call(
                "call-1",
                "abort",
                json!({"reason": "done"}),
            )),
            &["abort"],
        ))
        .unwrap();
        let AgentRunStep::CallTools { calls: expected } = run.next_step().unwrap() else {
            panic!("expected tool call");
        };
        let workspace = AgentWorkspace::default();
        let checkpoint = serde_json::to_value(AgentCheckpoint {
            run: &run,
            workspace: &workspace,
        })
        .unwrap();

        let mut resumed: OwnedAgentCheckpoint = serde_json::from_value(checkpoint).unwrap();
        let AgentRunStep::CallTools { calls } = resumed.run.next_step().unwrap() else {
            panic!("expected replayed tool call");
        };
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool_call.id, expected[0].tool_call.id);
        assert_eq!(
            calls[0].tool_call.function.name,
            expected[0].tool_call.function.name
        );
        assert_eq!(
            calls[0].tool_call.function.arguments,
            expected[0].tool_call.function.arguments
        );
    }
}
