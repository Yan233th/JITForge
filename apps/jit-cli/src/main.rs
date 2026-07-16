use std::{
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, IsTerminal, Read, Write},
    path::PathBuf,
    process,
    time::Duration,
};

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use clap::{Args, Parser, Subcommand};
use jit_config::JitForgeConfig;
use jit_protocol::{
    ErrorResponse, InvocationRequest, InvocationResponse, IoFormat, JobAnswerRequest,
    JobInputAnswer, JobInputKind, JobResponse, JobStatus, MAX_INPUT_SAMPLE_BYTES,
    RegistrationRequest, RegistrationResponse, RevokeRequest, RevokeResponse, ToolExample,
    ToolListResponse, ToolSummaryResponse,
};
use reqwest::{RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use tokio::time::{Instant, sleep};
use uuid::Uuid;

const DEFAULT_SERVER: &str = "http://127.0.0.1:8080";

#[derive(Debug, Parser)]
#[command(
    name = "jit",
    version,
    about = "Synthesize and call remote Unix-style tools"
)]
struct Cli {
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[arg(long, global = true, env = "JITFORGE_SERVER")]
    server: Option<String>,

    #[arg(long, global = true, env = "JITFORGE_TOKEN", hide_env_values = true)]
    token: Option<String>,

    #[arg(long, global = true, help = "Emit protocol responses as JSON")]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Register a new immutable candidate version. Piped stdin is used as an input sample.
    Register {
        name: String,
        #[arg(value_name = "INTENT")]
        intent: String,

        #[arg(long = "example", value_name = "INPUT => OUTPUT")]
        examples: Vec<String>,

        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        input_format: String,

        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        output_format: String,

        #[arg(long, help = "Return after the synthesis job is accepted")]
        no_wait: bool,

        #[arg(long, default_value = "10m", value_parser = parse_duration)]
        timeout: Duration,
    },

    /// Invoke a validated tool and forward its standard streams.
    Call {
        #[arg(value_name = "NAME[@REVISION]")]
        tool: String,

        #[arg(long, conflicts_with = "file")]
        input: Option<String>,

        #[arg(long, conflicts_with = "input")]
        file: Option<PathBuf>,

        #[arg(long, default_value = "text/plain")]
        content_type: String,

        #[arg(long, default_value = "5s", value_parser = parse_duration)]
        timeout: Duration,

        #[arg(last = true, value_name = "TOOL_ARGS")]
        args: Vec<String>,
    },

    /// Show a synthesis job without changing it.
    Status { job_id: String },

    /// Answer a clarification or approve a source for a suspended synthesis job.
    Answer {
        job_id: String,

        #[arg(value_name = "TEXT", conflicts_with_all = ["approve", "reject"])]
        answer: Option<String>,

        #[arg(long, conflicts_with_all = ["answer", "reject"])]
        approve: bool,

        #[arg(long, conflicts_with_all = ["answer", "approve"])]
        reject: bool,

        #[arg(long, requires = "reject")]
        reason: Option<String>,
    },

    /// List callable tools, optionally filtering by name or description.
    #[command(visible_alias = "ls")]
    List {
        #[arg(value_name = "QUERY")]
        query: Option<String>,

        #[command(flatten)]
        options: ToolListOptions,
    },

    /// Search callable tools by name or description.
    Search {
        #[arg(value_name = "QUERY")]
        query: String,

        #[command(flatten)]
        options: ToolListOptions,
    },

    /// Show a tool's selected version, contract, assumptions and validation.
    Inspect {
        #[arg(value_name = "NAME[@REVISION]")]
        tool: String,
    },

    /// Revoke a published version and stop it from being called.
    Revoke {
        #[arg(value_name = "NAME@REVISION")]
        tool: String,

        #[arg(long, value_name = "TEXT")]
        reason: String,
    },
}

#[derive(Debug, Args)]
struct ToolListOptions {
    #[arg(long, help = "Include tools without a ready stable revision")]
    all: bool,

    #[arg(long, help = "Print only tool names")]
    names_only: bool,

    #[arg(long, default_value_t = 50)]
    limit: u32,

    #[arg(long, default_value_t = 0)]
    offset: u64,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let json_errors = cli.json;
    let exit_code = match run(&cli).await {
        Ok(exit_code) => exit_code,
        Err(error) => {
            if json_errors {
                let body = ErrorResponse {
                    code: error.code.clone(),
                    message: error.message.clone(),
                    request_id: error.request_id.clone(),
                    details: error.details.clone(),
                };
                eprintln!(
                    "{}",
                    serde_json::to_string(&body).unwrap_or_else(|_| error.to_string())
                );
            } else {
                eprintln!("jit: {error}");
            }
            error.exit_code
        }
    };
    process::exit(exit_code);
}

async fn run(cli: &Cli) -> CliResult<i32> {
    let config = JitForgeConfig::load(cli.config.as_deref())
        .map_err(|error| CliFailure::local(78, "invalid_config", error.to_string()))?;
    let token = cli
        .token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or(config.auth.token.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CliFailure::local(
                77,
                "missing_token",
                "client token is required via --token, JITFORGE_TOKEN, or configuration",
            )
        })?;
    let client = reqwest::Client::builder().build().map_err(|error| {
        CliFailure::internal(format!("failed to initialize HTTP client: {error}"))
    })?;
    let server = cli
        .server
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or(config.client.server.as_deref())
        .unwrap_or(DEFAULT_SERVER)
        .trim_end_matches('/');

    match &cli.command {
        Command::Register {
            name,
            intent,
            examples,
            input_format,
            output_format,
            no_wait,
            timeout,
        } => {
            let input_samples = read_registration_input_samples()?;
            let request = RegistrationRequest {
                intent: intent.clone(),
                input_format: parse_io_format(input_format)?,
                output_format: parse_io_format(output_format)?,
                examples: examples
                    .iter()
                    .map(|value| parse_example(value))
                    .collect::<CliResult<_>>()?,
                input_samples,
            };
            let response = authenticated(
                client
                    .post(format!("{server}/v1/tools/{name}/registrations"))
                    .header("Idempotency-Key", Uuid::now_v7().to_string())
                    .json(&request),
                token,
            )
            .send()
            .await
            .map_err(CliFailure::transport)?;
            let registration = decode_response::<RegistrationResponse>(response).await?;

            if *no_wait {
                if cli.json {
                    print_json(&registration)?;
                } else {
                    println!("{}", registration.job_id);
                    eprintln!(
                        "jit: queued {}@{}",
                        registration.tool, registration.revision
                    );
                }
                return Ok(0);
            }

            if !cli.json {
                eprintln!("jit: job {}", registration.job_id);
            }

            let job = wait_for_job(
                &client,
                server,
                token,
                &registration.job_id,
                *timeout,
                !cli.json,
            )
            .await?;
            if cli.json {
                print_json(&job)?;
            }
            match job.status {
                JobStatus::Ready => {
                    if !cli.json {
                        println!("{}@{}", job.tool, job.revision);
                    }
                    Ok(0)
                }
                JobStatus::Rejected => Err(CliFailure::job_rejected(&job)),
                JobStatus::AwaitingInput => {
                    if !cli.json {
                        println!("{}", job.job_id);
                        if let Some(input) = &job.pending_input {
                            eprintln!("jit: awaiting input: {}", input.prompt);
                            eprintln!("jit: answer with `jit answer {} ...`", job.job_id);
                        }
                    }
                    Ok(75)
                }
                _ => Err(CliFailure::internal(
                    "job polling ended before a terminal state".to_owned(),
                )),
            }
        }
        Command::Call {
            tool,
            input,
            file,
            content_type,
            timeout,
            args,
        } => {
            if *timeout > Duration::from_secs(30) || timeout.is_zero() {
                return Err(CliFailure::local(
                    64,
                    "invalid_timeout",
                    "call timeout must be greater than zero and at most 30s",
                ));
            }
            let (name, revision) = parse_tool_reference(tool)?;
            let stdin = read_call_input(input.as_deref(), file.as_ref())?;
            if stdin.len() > 4 * 1024 * 1024 {
                return Err(CliFailure::local(
                    64,
                    "input_too_large",
                    "call input exceeds the 4 MiB limit",
                ));
            }
            let request = InvocationRequest {
                revision,
                args: args.clone(),
                content_type: content_type.clone(),
                stdin_base64: BASE64.encode(stdin),
                timeout_ms: Some(timeout.as_millis() as u64),
            };
            let response = authenticated(
                client
                    .post(format!("{server}/v1/tools/{name}/invocations"))
                    .json(&request),
                token,
            )
            .send()
            .await
            .map_err(CliFailure::transport)?;
            let response = decode_response::<InvocationResponse>(response).await?;

            if cli.json {
                print_json(&response)?;
            } else {
                let stdout = BASE64.decode(response.stdout_base64).map_err(|error| {
                    CliFailure::internal(format!(
                        "server returned invalid stdout encoding: {error}"
                    ))
                })?;
                let stderr = BASE64.decode(response.stderr_base64).map_err(|error| {
                    CliFailure::internal(format!(
                        "server returned invalid stderr encoding: {error}"
                    ))
                })?;
                io::stdout()
                    .lock()
                    .write_all(&stdout)
                    .map_err(CliFailure::io)?;
                io::stderr()
                    .lock()
                    .write_all(&stderr)
                    .map_err(CliFailure::io)?;
            }
            Ok(response.exit_code.clamp(0, 125))
        }
        Command::Status { job_id } => {
            let job = fetch_job(&client, server, token, job_id).await?;
            if cli.json {
                print_json(&job)?;
            } else {
                println!(
                    "{} {} {} {}@{}",
                    job.job_id,
                    job.status.as_str(),
                    job.stage.as_str(),
                    job.tool,
                    job.revision
                );
                if let Some(error) = job.error {
                    eprintln!("jit: {}: {}", error.code, error.message);
                }
                if let Some(input) = job.pending_input {
                    eprintln!("jit: awaiting input: {}", input.prompt);
                }
            }
            Ok(0)
        }
        Command::Answer {
            job_id,
            answer,
            approve,
            reject,
            reason,
        } => {
            let job = fetch_job(&client, server, token, job_id).await?;
            let pending = job.pending_input.as_ref().ok_or_else(|| {
                CliFailure::local(65, "job_input_not_pending", "job has no pending input")
            })?;
            let answer = match (answer, approve, reject) {
                (Some(text), false, false) => JobInputAnswer::Text { text: text.clone() },
                (None, true, false) => JobInputAnswer::Approve,
                (None, false, true) => JobInputAnswer::Reject {
                    reason: reason.clone(),
                },
                _ => {
                    return Err(CliFailure::local(
                        64,
                        "answer_required",
                        "provide TEXT, --approve, or --reject",
                    ));
                }
            };
            let answered = submit_job_answer(
                &client,
                server,
                token,
                job_id,
                JobAnswerRequest {
                    input_id: pending.id.clone(),
                    answer,
                },
            )
            .await?;
            if cli.json {
                print_json(&answered)?;
            } else {
                println!("{}", answered.job_id);
                eprintln!("jit: answer accepted; synthesis re-queued");
            }
            Ok(0)
        }
        Command::List { query, options } => {
            list_tools(
                &client,
                server,
                token,
                query.as_deref().unwrap_or_default(),
                options,
                cli.json,
            )
            .await
        }
        Command::Search { query, options } => {
            list_tools(&client, server, token, query, options, cli.json).await
        }
        Command::Inspect { tool } => {
            let (name, revision) = parse_tool_reference(tool)?;
            let mut request = authenticated(client.get(format!("{server}/v1/tools/{name}")), token);
            if let Some(revision) = revision {
                request = request.query(&[("revision", revision)]);
            }
            let response = request.send().await.map_err(CliFailure::transport)?;
            let tool = decode_response::<ToolSummaryResponse>(response).await?;
            if cli.json {
                print_json(&tool)?;
            } else {
                print_tool_summary(&tool);
            }
            Ok(0)
        }
        Command::Revoke { tool, reason } => {
            let (name, revision) = parse_tool_reference(tool)?;
            let revision = revision.ok_or_else(|| {
                CliFailure::local(
                    64,
                    "revision_required",
                    "revoke requires an explicit NAME@REVISION",
                )
            })?;
            let response = authenticated(
                client
                    .post(format!(
                        "{server}/v1/tools/{name}/versions/{revision}/revoke"
                    ))
                    .json(&RevokeRequest {
                        reason: reason.clone(),
                    }),
                token,
            )
            .send()
            .await
            .map_err(CliFailure::transport)?;
            let response = decode_response::<RevokeResponse>(response).await?;
            if cli.json {
                print_json(&response)?;
            } else {
                println!("{}@{}", response.tool, response.revision);
                match response.stable_revision {
                    Some(stable) => eprintln!("jit: revoked; stable is now {name}@{stable}"),
                    None => eprintln!("jit: revoked; {name} has no callable stable version"),
                }
            }
            Ok(0)
        }
    }
}

async fn list_tools(
    client: &reqwest::Client,
    server: &str,
    token: &str,
    query: &str,
    options: &ToolListOptions,
    json: bool,
) -> CliResult<i32> {
    if query.len() > 256 {
        return Err(CliFailure::local(
            64,
            "invalid_query",
            "tool search query must not exceed 256 bytes",
        ));
    }
    if !(1..=100).contains(&options.limit) {
        return Err(CliFailure::local(
            64,
            "invalid_limit",
            "list limit must be between 1 and 100",
        ));
    }
    let parameters = [
        ("query", query.to_owned()),
        ("include_unready", options.all.to_string()),
        ("limit", options.limit.to_string()),
        ("offset", options.offset.to_string()),
    ];
    let response = authenticated(client.get(format!("{server}/v1/tools")), token)
        .query(&parameters)
        .send()
        .await
        .map_err(CliFailure::transport)?;
    let response = decode_response::<ToolListResponse>(response).await?;
    if json {
        print_json(&response)?;
    } else {
        print_tool_list(&response, options.names_only);
        if let Some(next_offset) = response.next_offset {
            eprintln!("jit: more tools available; use --offset {next_offset}");
        }
    }
    Ok(0)
}

async fn wait_for_job(
    client: &reqwest::Client,
    server: &str,
    token: &str,
    job_id: &str,
    timeout: Duration,
    show_progress: bool,
) -> CliResult<JobResponse> {
    let deadline = Instant::now() + timeout;
    let mut previous_stage = None;
    loop {
        let job = fetch_job(client, server, token, job_id).await?;
        if show_progress && previous_stage != Some(job.stage) {
            eprintln!("jit: {}", job.stage.as_str());
            previous_stage = Some(job.stage);
        }
        if job.status.is_terminal() {
            return Ok(job);
        }
        if job.status == JobStatus::AwaitingInput {
            if show_progress && let Some(request) = prompt_for_job_input(&job)? {
                submit_job_answer(client, server, token, job_id, request).await?;
                previous_stage = None;
                continue;
            }
            return Ok(job);
        }
        if Instant::now() >= deadline {
            return Err(CliFailure::local(
                124,
                "wait_timeout",
                "timed out waiting for synthesis; the server job is still running",
            ));
        }
        sleep(Duration::from_millis(500)).await;
    }
}

async fn submit_job_answer(
    client: &reqwest::Client,
    server: &str,
    token: &str,
    job_id: &str,
    request: JobAnswerRequest,
) -> CliResult<JobResponse> {
    let response = authenticated(
        client
            .post(format!("{server}/v1/jobs/{job_id}"))
            .json(&request),
        token,
    )
    .send()
    .await
    .map_err(CliFailure::transport)?;
    decode_response(response).await
}

fn prompt_for_job_input(job: &JobResponse) -> CliResult<Option<JobAnswerRequest>> {
    let Some(pending) = &job.pending_input else {
        return Err(CliFailure::internal(
            "awaiting_input job omitted pending_input".to_owned(),
        ));
    };
    let mut tty = match OpenOptions::new().read(true).write(true).open("/dev/tty") {
        Ok(tty) => tty,
        Err(_) => return Ok(None),
    };
    let reader_file = tty.try_clone().map_err(CliFailure::io)?;
    let mut reader = BufReader::new(reader_file);
    writeln!(tty, "\njit: {}", pending.prompt).map_err(CliFailure::io)?;
    let show_context = match &pending.context {
        serde_json::Value::Null => false,
        serde_json::Value::Object(object) => !object.is_empty(),
        _ => true,
    };
    if show_context {
        let context = serde_json::to_string_pretty(&pending.context)
            .map_err(|error| CliFailure::internal(error.to_string()))?;
        writeln!(tty, "{context}").map_err(CliFailure::io)?;
    }
    for (index, choice) in pending.choices.iter().enumerate() {
        write!(tty, "  {}. {}", index + 1, choice.label).map_err(CliFailure::io)?;
        if let Some(description) = &choice.description {
            write!(tty, " — {description}").map_err(CliFailure::io)?;
        }
        writeln!(tty).map_err(CliFailure::io)?;
    }

    let answer = match pending.kind {
        JobInputKind::SourceApproval => loop {
            write!(tty, "Approve this source? [y/N] ").map_err(CliFailure::io)?;
            tty.flush().map_err(CliFailure::io)?;
            let mut line = String::new();
            if reader.read_line(&mut line).map_err(CliFailure::io)? == 0 {
                return Ok(None);
            }
            match line.trim().to_ascii_lowercase().as_str() {
                "y" | "yes" => break JobInputAnswer::Approve,
                "" | "n" | "no" => break JobInputAnswer::Reject { reason: None },
                _ => writeln!(tty, "Please answer y or n.").map_err(CliFailure::io)?,
            }
        },
        JobInputKind::Clarification => loop {
            write!(tty, "Answer: ").map_err(CliFailure::io)?;
            tty.flush().map_err(CliFailure::io)?;
            let mut line = String::new();
            if reader.read_line(&mut line).map_err(CliFailure::io)? == 0 {
                return Ok(None);
            }
            let raw = line.trim();
            if raw.is_empty() {
                writeln!(tty, "Answer must not be empty.").map_err(CliFailure::io)?;
                continue;
            }
            let text = raw
                .parse::<usize>()
                .ok()
                .and_then(|index| index.checked_sub(1))
                .and_then(|index| pending.choices.get(index))
                .map(|choice| choice.value.clone())
                .unwrap_or_else(|| raw.to_owned());
            break JobInputAnswer::Text { text };
        },
    };
    Ok(Some(JobAnswerRequest {
        input_id: pending.id.clone(),
        answer,
    }))
}

async fn fetch_job(
    client: &reqwest::Client,
    server: &str,
    token: &str,
    job_id: &str,
) -> CliResult<JobResponse> {
    let response = authenticated(client.get(format!("{server}/v1/jobs/{job_id}")), token)
        .send()
        .await
        .map_err(CliFailure::transport)?;
    decode_response(response).await
}

fn authenticated(request: RequestBuilder, token: &str) -> RequestBuilder {
    request.bearer_auth(token)
}

fn parse_duration(value: &str) -> Result<Duration, String> {
    humantime::parse_duration(value).map_err(|error| error.to_string())
}

fn parse_io_format(value: &str) -> CliResult<IoFormat> {
    IoFormat::parse(value).ok_or_else(|| {
        CliFailure::local(
            64,
            "invalid_format",
            format!("unsupported I/O format {value:?}"),
        )
    })
}

fn parse_example(value: &str) -> CliResult<ToolExample> {
    let (input, output) = value.split_once("=>").ok_or_else(|| {
        CliFailure::local(
            64,
            "invalid_example",
            "examples must use the form 'INPUT => OUTPUT'",
        )
    })?;
    Ok(ToolExample {
        input: input.trim().to_owned(),
        output: output.trim().to_owned(),
    })
}

fn parse_tool_reference(value: &str) -> CliResult<(&str, Option<u64>)> {
    match value.rsplit_once('@') {
        Some((name, revision)) if !name.is_empty() && !revision.is_empty() => {
            let revision = revision.parse::<u64>().map_err(|_| {
                CliFailure::local(
                    64,
                    "invalid_tool_reference",
                    "tool revision must be a positive integer",
                )
            })?;
            if revision == 0 {
                return Err(CliFailure::local(
                    64,
                    "invalid_tool_reference",
                    "tool revision must be a positive integer",
                ));
            }
            Ok((name, Some(revision)))
        }
        Some(_) => Err(CliFailure::local(
            64,
            "invalid_tool_reference",
            "tool reference must use NAME@REVISION",
        )),
        None => Ok((value, None)),
    }
}

fn read_call_input(input: Option<&str>, file: Option<&PathBuf>) -> CliResult<Vec<u8>> {
    if let Some(input) = input {
        return Ok(input.as_bytes().to_vec());
    }
    if let Some(path) = file {
        return fs::read(path).map_err(|error| {
            CliFailure::local(
                66,
                "input_read_failed",
                format!("failed to read {}: {error}", path.display()),
            )
        });
    }
    if io::stdin().is_terminal() {
        return Ok(Vec::new());
    }

    let mut bytes = Vec::new();
    io::stdin()
        .read_to_end(&mut bytes)
        .map_err(CliFailure::io)?;
    Ok(bytes)
}

fn read_registration_input_samples() -> CliResult<Vec<String>> {
    if io::stdin().is_terminal() {
        return Ok(Vec::new());
    }
    read_input_sample(io::stdin().lock())
}

fn read_input_sample(reader: impl Read) -> CliResult<Vec<String>> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_INPUT_SAMPLE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(CliFailure::io)?;
    if bytes.len() > MAX_INPUT_SAMPLE_BYTES {
        return Err(CliFailure::local(
            64,
            "input_sample_too_large",
            "registration input sample exceeds the 256 KiB limit",
        ));
    }
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let sample = String::from_utf8(bytes).map_err(|_| {
        CliFailure::local(
            64,
            "invalid_input_sample",
            "registration input sample must be UTF-8 text",
        )
    })?;
    Ok(vec![sample])
}

async fn decode_response<T>(response: reqwest::Response) -> CliResult<T>
where
    T: DeserializeOwned,
{
    let status = response.status();
    if status.is_success() {
        return response.json::<T>().await.map_err(|error| {
            CliFailure::internal(format!(
                "server returned an invalid success response: {error}"
            ))
        });
    }

    let body = response.json::<ErrorResponse>().await.ok();
    match body {
        Some(body) => Err(CliFailure::from_api(status, body)),
        None => Err(CliFailure::local(
            status_to_exit(status),
            "http_error",
            format!("server returned HTTP {status}"),
        )),
    }
}

fn print_json<T: serde::Serialize>(value: &T) -> CliResult<()> {
    println!(
        "{}",
        serde_json::to_string(value).map_err(|error| {
            CliFailure::internal(format!("failed to encode JSON output: {error}"))
        })?
    );
    Ok(())
}

fn print_tool_summary(tool: &ToolSummaryResponse) {
    println!(
        "{}@{} {}",
        tool.tool,
        tool.selected.revision,
        tool.selected.status.as_str()
    );
    println!("stable: {}", format_revision(tool.stable_revision));
    println!("latest: {}", tool.latest_revision);
    println!("input: {}", tool.selected.input_format.as_str());
    println!("output: {}", tool.selected.output_format.as_str());
    println!("intent: {}", tool.selected.requested_intent);
    println!("description: {}", tool.selected.description);
    if !tool.selected.assumptions.is_empty() {
        println!("assumptions:");
        for assumption in &tool.selected.assumptions {
            println!("- {assumption}");
        }
    }
    if let Some(digest) = &tool.selected.artifact_digest {
        println!("artifact: {digest}");
    }
    if let Some(error) = &tool.selected.error {
        println!("error: {}: {}", error.code, error.message);
    }
}

fn print_tool_list(response: &ToolListResponse, names_only: bool) {
    if names_only {
        for tool in &response.tools {
            println!("{}", tool.tool);
        }
        return;
    }
    if response.tools.is_empty() {
        return;
    }
    let name_width = response
        .tools
        .iter()
        .map(|tool| tool.tool.chars().count())
        .max()
        .unwrap_or(4)
        .max(4);
    println!(
        "{:<name_width$}  {:>8}  {:<11}  {:<9}  DESCRIPTION",
        "NAME", "REVISION", "STATUS", "I/O"
    );
    for tool in &response.tools {
        let io = format!(
            "{}/{}",
            tool.input_format.as_str(),
            tool.output_format.as_str()
        );
        println!(
            "{:<name_width$}  {:>8}  {:<11}  {:<9}  {}",
            tool.tool,
            tool.selected_revision,
            tool.status.as_str(),
            io,
            single_line(&tool.description, 96)
        );
    }
}

fn single_line(value: &str, limit: usize) -> String {
    let sanitized: String = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    let normalized = sanitized.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut characters = normalized.chars();
    let abbreviated: String = characters.by_ref().take(limit).collect();
    if characters.next().is_some() {
        format!("{abbreviated}…")
    } else {
        abbreviated
    }
}

fn format_revision(revision: Option<u64>) -> String {
    revision
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_owned())
}

#[derive(Debug)]
struct CliFailure {
    exit_code: i32,
    code: String,
    message: String,
    request_id: String,
    details: Option<serde_json::Value>,
}

type CliResult<T> = Result<T, CliFailure>;

impl CliFailure {
    fn local(exit_code: i32, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            exit_code,
            code: code.into(),
            message: message.into(),
            request_id: "local".to_owned(),
            details: None,
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::local(70, "internal_error", message)
    }

    fn transport(error: reqwest::Error) -> Self {
        Self::local(69, "service_unavailable", error.to_string())
    }

    fn io(error: io::Error) -> Self {
        Self::local(70, "io_error", error.to_string())
    }

    fn from_api(status: StatusCode, body: ErrorResponse) -> Self {
        let exit_code = match body.code.as_str() {
            "unauthorized" => 77,
            "tool_not_found" | "version_not_found" | "job_not_found" => 127,
            "worker_unavailable" => 69,
            "execution_timeout" => 124,
            _ => status_to_exit(status),
        };
        Self {
            exit_code,
            code: body.code,
            message: body.message,
            request_id: body.request_id,
            details: body.details,
        }
    }

    fn job_rejected(job: &JobResponse) -> Self {
        match &job.error {
            Some(error) => Self {
                exit_code: 1,
                code: error.code.clone(),
                message: error.message.clone(),
                request_id: job.job_id.clone(),
                details: error.details.clone(),
            },
            None => Self::local(1, "synthesis_rejected", "synthesis was rejected"),
        }
    }
}

impl std::fmt::Display for CliFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

fn status_to_exit(status: StatusCode) -> i32 {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => 77,
        StatusCode::NOT_FOUND => 127,
        StatusCode::REQUEST_TIMEOUT | StatusCode::GATEWAY_TIMEOUT => 124,
        StatusCode::SERVICE_UNAVAILABLE | StatusCode::BAD_GATEWAY => 69,
        _ if status.is_client_error() => 64,
        _ => 70,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tool_references() {
        assert_eq!(parse_tool_reference("slugify").unwrap(), ("slugify", None));
        assert_eq!(
            parse_tool_reference("slugify@12").unwrap(),
            ("slugify", Some(12))
        );
        assert!(parse_tool_reference("slugify@0").is_err());
        assert!(parse_tool_reference("slugify@latest").is_err());
    }

    #[test]
    fn parses_examples_at_the_first_arrow() {
        let example = parse_example("a => b => c").unwrap();
        assert_eq!(example.input, "a");
        assert_eq!(example.output, "b => c");
    }

    #[test]
    fn reads_piped_registration_input_as_an_unpaired_sample() {
        let samples = read_input_sample("Architecture: x86_64\n".as_bytes()).unwrap();
        assert_eq!(samples, vec!["Architecture: x86_64\n"]);
    }

    #[test]
    fn rejects_oversized_registration_input_samples() {
        let sample = vec![b'x'; MAX_INPUT_SAMPLE_BYTES + 1];
        let failure = read_input_sample(sample.as_slice()).unwrap_err();
        assert_eq!(failure.code, "input_sample_too_large");
    }

    #[test]
    fn rejects_non_utf8_registration_input_samples() {
        let failure = read_input_sample([0xff].as_slice()).unwrap_err();
        assert_eq!(failure.code, "invalid_input_sample");
    }

    #[test]
    fn maps_api_failures_to_stable_exit_codes() {
        let failure = CliFailure::from_api(
            StatusCode::UNAUTHORIZED,
            ErrorResponse::new("unauthorized", "bad token", "req_1"),
        );
        assert_eq!(failure.exit_code, 77);
    }

    #[test]
    fn local_json_errors_have_a_stable_shape() {
        let failure = CliFailure::local(64, "invalid_input", "bad");
        let value = serde_json::json!({
            "code": failure.code,
            "message": failure.message,
            "request_id": failure.request_id,
        });
        assert_eq!(value["code"], "invalid_input");
    }

    #[test]
    fn list_has_an_ls_alias() {
        let cli = Cli::try_parse_from(["jit", "ls"]).unwrap();
        assert!(matches!(cli.command, Command::List { .. }));
    }

    #[test]
    fn parses_explicit_revision_revocation() {
        let cli = Cli::try_parse_from([
            "jit",
            "revoke",
            "log-analysis@1",
            "--reason",
            "incorrect implementation",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Revoke { tool, reason }
                if tool == "log-analysis@1" && reason == "incorrect implementation"
        ));
    }

    #[test]
    fn list_descriptions_are_single_line_and_bounded() {
        assert_eq!(single_line("one\n two\tthree", 20), "one two three");
        assert_eq!(single_line("safe\u{1b}[31mtext", 20), "safe [31mtext");
        assert_eq!(single_line("abcdef", 3), "abc…");
    }
}
