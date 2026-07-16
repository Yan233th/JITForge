use jit_domain::ToolVersionStatus;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod worker {
    tonic::include_proto!("jitforge.worker.v1");
}

pub const MAX_INPUT_SAMPLES: usize = 8;
pub const MAX_INPUT_SAMPLE_BYTES: usize = 256 * 1024;
pub const MAX_INPUT_SAMPLES_TOTAL_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub service: String,
    pub version: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReadyResponse {
    pub status: String,
    pub database: bool,
    pub worker: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IoFormat {
    #[default]
    Text,
    Json,
}

impl IoFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "text" => Some(Self::Text),
            "json" => Some(Self::Json),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolExample {
    pub input: String,
    pub output: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RegistrationRequest {
    #[serde(alias = "description")]
    pub intent: String,
    #[serde(default)]
    pub input_format: IoFormat,
    #[serde(default)]
    pub output_format: IoFormat,
    #[serde(default)]
    pub examples: Vec<ToolExample>,
    #[serde(default)]
    pub input_samples: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RevokeRequest {
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RevokeResponse {
    pub tool: String,
    pub revision: u64,
    pub status: ToolVersionStatus,
    pub stable_revision: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SessionLoginRequest {
    pub token: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SessionResponse {
    pub csrf_token: String,
    pub expires_at: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    AwaitingInput,
    Ready,
    Rejected,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::AwaitingInput => "awaiting_input",
            Self::Ready => "ready",
            Self::Rejected => "rejected",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "awaiting_input" => Some(Self::AwaitingInput),
            "ready" => Some(Self::Ready),
            "rejected" => Some(Self::Rejected),
            _ => None,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Ready | Self::Rejected)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStage {
    Queued,
    Contract,
    Synthesizing,
    Building,
    Validating,
    Repairing,
    AwaitingInput,
    Complete,
}

impl JobStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Contract => "contract",
            Self::Synthesizing => "synthesizing",
            Self::Building => "building",
            Self::Validating => "validating",
            Self::Repairing => "repairing",
            Self::AwaitingInput => "awaiting_input",
            Self::Complete => "complete",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "contract" => Some(Self::Contract),
            "synthesizing" => Some(Self::Synthesizing),
            "building" => Some(Self::Building),
            "validating" => Some(Self::Validating),
            "repairing" => Some(Self::Repairing),
            "awaiting_input" => Some(Self::AwaitingInput),
            "complete" => Some(Self::Complete),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RegistrationResponse {
    pub tool: String,
    pub revision: u64,
    pub status: ToolVersionStatus,
    pub job_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobInputKind {
    Clarification,
    SourceApproval,
    ExampleCorrection,
}

impl JobInputKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clarification => "clarification",
            Self::SourceApproval => "source_approval",
            Self::ExampleCorrection => "example_correction",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "clarification" => Some(Self::Clarification),
            "source_approval" => Some(Self::SourceApproval),
            "example_correction" => Some(Self::ExampleCorrection),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobInputChoice {
    pub value: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PendingJobInput {
    pub id: String,
    pub kind: JobInputKind,
    pub prompt: String,
    #[serde(default)]
    pub choices: Vec<JobInputChoice>,
    #[serde(default)]
    pub context: Value,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobInputAnswer {
    Text {
        text: String,
    },
    Approve,
    Reject {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobAnswerRequest {
    pub input_id: String,
    pub answer: JobInputAnswer,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CancelJobRequest {
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobResponse {
    pub job_id: String,
    pub tool: String,
    pub revision: u64,
    pub status: JobStatus,
    pub stage: JobStage,
    pub version_status: ToolVersionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JobError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_input: Option<PendingJobInput>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolVersionSummary {
    pub revision: u64,
    pub requested_intent: String,
    pub description: String,
    pub status: ToolVersionStatus,
    pub input_format: IoFormat,
    pub output_format: IoFormat,
    pub assumptions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contract: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation_summary: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JobError>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolSummaryResponse {
    pub tool: String,
    pub stable_revision: Option<u64>,
    pub latest_revision: u64,
    pub selected: ToolVersionSummary,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolListItem {
    pub tool: String,
    pub stable_revision: Option<u64>,
    pub latest_revision: u64,
    pub selected_revision: u64,
    pub description: String,
    pub status: ToolVersionStatus,
    pub input_format: IoFormat,
    pub output_format: IoFormat,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolListResponse {
    pub tools: Vec<ToolListItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolVersionListItem {
    pub revision: u64,
    pub description: String,
    pub status: ToolVersionStatus,
    pub input_format: IoFormat,
    pub output_format: IoFormat,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JobError>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolVersionListResponse {
    pub tool: String,
    pub stable_revision: Option<u64>,
    pub latest_revision: u64,
    pub versions: Vec<ToolVersionListItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobListResponse {
    pub jobs: Vec<JobResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolArtifactManifest {
    pub format_version: u32,
    pub runtime: String,
    pub input_format: IoFormat,
    pub output_format: IoFormat,
    pub source_sha256: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub http_capabilities: Vec<HttpCapabilityGrant>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
}

impl HttpMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HttpCapability {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub method: HttpMethod,
    pub path_prefix: String,
    #[serde(default)]
    pub query_keys: Vec<String>,
    pub purpose: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HttpCapabilityGrant {
    pub approval_hash: String,
    pub capability: HttpCapability,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HttpFixture {
    pub request_url: String,
    pub response_url: String,
    pub status: u16,
    pub content_type: String,
    pub body: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HttpCapabilityApproval {
    pub capability_hash: String,
    pub capability: HttpCapability,
    pub status: String,
    pub approved_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HttpCapabilityApprovalList {
    pub approvals: Vec<HttpCapabilityApproval>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolArtifactTestCase {
    pub name: String,
    pub args: Vec<String>,
    pub stdin: String,
    pub expected_stdout: String,
    pub expected_exit_code: i32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolArtifactResponse {
    pub tool: String,
    pub revision: u64,
    pub digest: String,
    pub manifest: ToolArtifactManifest,
    pub source: String,
    pub tests: Vec<ToolArtifactTestCase>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_accepts_legacy_description_as_intent() {
        let request: RegistrationRequest = serde_json::from_value(serde_json::json!({
            "description": "make a slug"
        }))
        .unwrap();
        assert_eq!(request.intent, "make a slug");
        let encoded = serde_json::to_value(request).unwrap();
        assert_eq!(encoded["intent"], "make a slug");
        assert!(encoded.get("description").is_none());
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InvocationRequest {
    #[serde(default)]
    pub revision: Option<u64>,
    #[serde(default)]
    pub args: Vec<String>,
    pub content_type: String,
    pub stdin_base64: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InvocationResponse {
    pub invocation_id: String,
    pub resolved_revision: u64,
    pub exit_code: i32,
    pub stdout_base64: String,
    pub stderr_base64: String,
    pub duration_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ErrorResponse {
    pub code: String,
    pub message: String,
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl ErrorResponse {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        request_id: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            request_id: request_id.into(),
            details: None,
        }
    }
}
