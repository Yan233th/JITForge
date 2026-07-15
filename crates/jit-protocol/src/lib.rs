use jit_domain::ToolVersionStatus;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod worker {
    tonic::include_proto!("jitforge.worker.v1");
}

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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolExample {
    pub input: String,
    pub output: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RegistrationRequest {
    pub description: String,
    #[serde(default)]
    pub input_format: IoFormat,
    #[serde(default)]
    pub output_format: IoFormat,
    #[serde(default)]
    pub examples: Vec<ToolExample>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Ready,
    Rejected,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Ready => "ready",
            Self::Rejected => "rejected",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
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
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolVersionSummary {
    pub revision: u64,
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
