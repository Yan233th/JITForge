mod web;

use std::net::SocketAddr;

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{HeaderMap, HeaderName, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use jit_artifact::{ArtifactError, ArtifactStore};
use jit_config::{JitForgeConfig, nonempty_env};
use jit_domain::{ToolIntent, ToolName};
use jit_protocol::{
    CancelJobRequest, ErrorResponse, HealthResponse, InvocationRequest, InvocationResponse,
    JobAnswerRequest, JobStatus, MAX_INPUT_SAMPLE_BYTES, MAX_INPUT_SAMPLES,
    MAX_INPUT_SAMPLES_TOTAL_BYTES, ReadyResponse, RegistrationRequest, RevokeRequest,
    ToolArtifactManifest, ToolArtifactResponse, ToolArtifactTestCase,
    worker::{ExecuteRequest, runner_client::RunnerClient},
};
use jit_storage::{Registry, StorageError};
use serde::Deserialize;
use subtle::ConstantTimeEq;
use tokio::signal;
use tonic::{
    Code, Request as GrpcRequest,
    transport::{Channel, Endpoint},
};
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use ulid::Ulid;
use uuid::Uuid;

const DEFAULT_LISTEN_ADDR: &str = "127.0.0.1:8080";
const DEFAULT_DATABASE_URL: &str = "postgres://jitforge:jitforge@127.0.0.1:5432/jitforge";
const IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";

#[derive(Clone)]
struct AppState {
    registry: Registry,
    artifact_store: ArtifactStore,
    auth_token: String,
    worker: RunnerClient<Channel>,
    worker_token: String,
    sessions: web::SessionStore,
}

#[derive(Debug)]
struct Config {
    listen_addr: SocketAddr,
    database_url: String,
    auth_token: String,
    worker_endpoint: String,
    worker_token: String,
    artifact_dir: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let config = Config::from_env()?;
    let registry = Registry::connect(&config.database_url, 10)
        .await
        .context("failed to connect to PostgreSQL")?;
    registry
        .migrate()
        .await
        .context("failed to apply database migrations")?;

    let worker_channel = Endpoint::from_shared(config.worker_endpoint.clone())
        .context("JITFORGE_WORKER_ENDPOINT must be a valid URI")?
        .connect_lazy();
    let app = build_router(AppState {
        registry,
        artifact_store: ArtifactStore::new(&config.artifact_dir),
        auth_token: config.auth_token,
        worker: RunnerClient::new(worker_channel),
        worker_token: config.worker_token,
        sessions: web::SessionStore::default(),
    });
    let listener = tokio::net::TcpListener::bind(config.listen_addr)
        .await
        .with_context(|| format!("failed to bind {}", config.listen_addr))?;

    info!(listen_addr = %config.listen_addr, "jit-server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server failed")
}

impl Config {
    fn from_env() -> Result<Self> {
        let config = JitForgeConfig::load(None).context("failed to load JITForge configuration")?;
        let listen_addr = configured("JITFORGE_LISTEN_ADDR", config.server.listen_addr)
            .unwrap_or_else(|| DEFAULT_LISTEN_ADDR.to_owned())
            .parse::<SocketAddr>()
            .context("JITFORGE_LISTEN_ADDR must be a valid socket address")?;
        let database_url = configured("JITFORGE_DATABASE_URL", config.server.database_url)
            .unwrap_or_else(|| DEFAULT_DATABASE_URL.to_owned());
        let auth_token = configured("JITFORGE_TOKEN", config.auth.token)
            .context("client token is required in configuration or JITFORGE_TOKEN")?;
        let worker_token = configured("JITFORGE_WORKER_TOKEN", config.auth.worker_token)
            .context("worker token is required in configuration or JITFORGE_WORKER_TOKEN")?;
        Ok(Self {
            listen_addr,
            database_url,
            auth_token,
            worker_endpoint: configured("JITFORGE_WORKER_ENDPOINT", config.server.worker_endpoint)
                .unwrap_or_else(|| "http://127.0.0.1:50051".to_owned()),
            worker_token,
            artifact_dir: configured("JITFORGE_ARTIFACT_DIR", config.server.artifact_dir)
                .unwrap_or_else(|| ".data/artifacts".to_owned()),
        })
    }
}

fn configured(env_name: &str, file_value: Option<String>) -> Option<String> {
    nonempty_env(env_name).or_else(|| {
        file_value
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    })
}

fn build_router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/v1/tools", get(list_tools))
        .route("/v1/tools/{name}", get(inspect_tool))
        .route("/v1/tools/{name}/versions", get(list_tool_versions))
        .route(
            "/v1/tools/{name}/versions/{revision}/artifact",
            get(get_tool_artifact),
        )
        .route("/v1/tools/{name}/registrations", post(register_tool))
        .route(
            "/v1/tools/{name}/versions/{revision}/revoke",
            post(revoke_tool_version),
        )
        .route("/v1/tools/{name}/invocations", post(invoke_tool))
        .route("/v1/jobs", get(list_jobs))
        .route("/v1/jobs/{job_id}", get(get_job).post(answer_job_input))
        .route("/v1/jobs/{job_id}/cancel", post(cancel_job))
        .route("/v1/http-capabilities", get(list_http_capability_approvals))
        .route(
            "/v1/http-capabilities/{capability_hash}/revoke",
            post(revoke_http_capability),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));

    let request_id_header = HeaderName::from_static("x-request-id");
    Router::new()
        .route("/", get(web::root))
        .route("/ui", get(web::root))
        .route("/ui/", get(web::index))
        .route("/ui/app.css", get(web::css))
        .route("/ui/app.js", get(web::js))
        .route(
            "/v1/session",
            get(web::current_session)
                .post(web::login)
                .delete(web::logout),
        )
        .route("/healthz", get(health))
        .route("/readyz", get(ready))
        .merge(protected)
        .layer(DefaultBodyLimit::max(6 * 1024 * 1024))
        .layer(PropagateRequestIdLayer::new(request_id_header.clone()))
        .layer(SetRequestIdLayer::new(request_id_header, MakeRequestUuid))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_owned(),
        service: "jit-server".to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    })
}

async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    let database = state.registry.database_ready().await;
    let worker = state.registry.has_recent_worker().await.unwrap_or(false);
    let status = if database && worker {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(ReadyResponse {
            status: if status == StatusCode::OK {
                "ready".to_owned()
            } else {
                "not_ready".to_owned()
            },
            database,
            worker,
        }),
    )
}

async fn register_tool(
    State(state): State<AppState>,
    Path(raw_name): Path<String>,
    headers: HeaderMap,
    Json(request): Json<RegistrationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let name = raw_name
        .parse::<ToolName>()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    request
        .intent
        .parse::<ToolIntent>()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    if request.examples.len() > 32 {
        return Err(ApiError::bad_request("at most 32 examples are allowed"));
    }
    if request.input_samples.len() > MAX_INPUT_SAMPLES {
        return Err(ApiError::bad_request(format!(
            "at most {MAX_INPUT_SAMPLES} input samples are allowed"
        )));
    }
    if request
        .input_samples
        .iter()
        .any(|sample| sample.len() > MAX_INPUT_SAMPLE_BYTES)
        || request.input_samples.iter().map(String::len).sum::<usize>()
            > MAX_INPUT_SAMPLES_TOTAL_BYTES
    {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "input_samples_too_large",
            "input samples must be at most 256 KiB each and 1 MiB in total",
        ));
    }
    if request.input_format == jit_protocol::IoFormat::Json {
        for sample in &request.input_samples {
            serde_json::from_str::<serde_json::Value>(sample).map_err(|error| {
                ApiError::bad_request(format!("input sample is not valid JSON: {error}"))
            })?;
        }
    }
    let idempotency_key = headers
        .get(IDEMPOTENCY_KEY_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty() && value.len() <= 200)
        .ok_or_else(|| {
            ApiError::bad_request("Idempotency-Key header is required and must be 1-200 bytes")
        })?;

    let response = state
        .registry
        .register(&name, &request, idempotency_key)
        .await
        .map_err(ApiError::from_storage)?;
    info!(tool = %name, revision = response.revision, job_id = %response.job_id, "registration accepted");
    Ok((StatusCode::ACCEPTED, Json(response)))
}

async fn revoke_tool_version(
    State(state): State<AppState>,
    Path((raw_name, revision)): Path<(String, u64)>,
    Json(request): Json<RevokeRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let name = raw_name
        .parse::<ToolName>()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let reason = request.reason.trim();
    if reason.is_empty() || reason.len() > 4096 {
        return Err(ApiError::bad_request(
            "revocation reason must contain 1-4096 non-whitespace bytes",
        ));
    }
    if revision == 0 {
        return Err(ApiError::bad_request("tool revision must be positive"));
    }
    let response = state
        .registry
        .revoke_tool_version(&name, revision, reason)
        .await
        .map_err(ApiError::from_storage)?;
    info!(tool = %name, revision, "tool version revoked");
    Ok(Json(response))
}

async fn get_job(
    State(state): State<AppState>,
    Path(raw_job_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let job_id =
        Uuid::parse_str(&raw_job_id).map_err(|_| ApiError::bad_request("job ID must be a UUID"))?;
    let response = state
        .registry
        .get_job(job_id)
        .await
        .map_err(ApiError::from_storage)?;
    Ok(Json(response))
}

async fn answer_job_input(
    State(state): State<AppState>,
    Path(raw_job_id): Path<String>,
    Json(request): Json<JobAnswerRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let job_id =
        Uuid::parse_str(&raw_job_id).map_err(|_| ApiError::bad_request("job ID must be a UUID"))?;
    let response = state
        .registry
        .answer_job_input(job_id, &request)
        .await
        .map_err(ApiError::from_storage)?;
    Ok(Json(response))
}

async fn cancel_job(
    State(state): State<AppState>,
    Path(raw_job_id): Path<String>,
    Json(request): Json<CancelJobRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let job_id =
        Uuid::parse_str(&raw_job_id).map_err(|_| ApiError::bad_request("job ID must be a UUID"))?;
    state
        .registry
        .cancel_job(job_id, &request.reason)
        .await
        .map_err(ApiError::from_storage)?;
    let response = state
        .registry
        .get_job(job_id)
        .await
        .map_err(ApiError::from_storage)?;
    Ok(Json(response))
}

async fn list_http_capability_approvals(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiError> {
    let response = state
        .registry
        .list_http_capability_approvals()
        .await
        .map_err(ApiError::from_storage)?;
    Ok(Json(response))
}

async fn revoke_http_capability(
    State(state): State<AppState>,
    Path(capability_hash): Path<String>,
    Json(request): Json<RevokeRequest>,
) -> Result<impl IntoResponse, ApiError> {
    state
        .registry
        .revoke_http_capability(&capability_hash, &request.reason)
        .await
        .map_err(ApiError::from_storage)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Default, Deserialize)]
struct ListJobsQuery {
    status: Option<String>,
    limit: Option<u32>,
    offset: Option<u64>,
}

async fn list_jobs(
    State(state): State<AppState>,
    Query(query): Query<ListJobsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let (limit, offset) = validated_page(query.limit, query.offset)?;
    let status = query
        .status
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(|value| {
            JobStatus::parse(value)
                .ok_or_else(|| ApiError::bad_request(format!("unknown job status {value:?}")))
        })
        .transpose()?;
    let response = state
        .registry
        .list_jobs(status, limit, offset)
        .await
        .map_err(ApiError::from_storage)?;
    Ok(Json(response))
}

#[derive(Debug, Default, Deserialize)]
struct ListToolsQuery {
    #[serde(default)]
    query: String,
    #[serde(default)]
    include_unready: bool,
    limit: Option<u32>,
    offset: Option<u64>,
}

async fn list_tools(
    State(state): State<AppState>,
    Query(query): Query<ListToolsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    if query.query.len() > 256 {
        return Err(ApiError::bad_request(
            "tool search query must not exceed 256 bytes",
        ));
    }
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(ApiError::bad_request("limit must be between 1 and 100"));
    }
    let offset = query.offset.unwrap_or(0);
    if offset > i64::MAX as u64 {
        return Err(ApiError::bad_request("offset is too large"));
    }
    let response = state
        .registry
        .list_tools(&query.query, query.include_unready, limit, offset)
        .await
        .map_err(ApiError::from_storage)?;
    Ok(Json(response))
}

#[derive(Debug, Default, Deserialize)]
struct InspectQuery {
    revision: Option<u64>,
}

async fn inspect_tool(
    State(state): State<AppState>,
    Path(raw_name): Path<String>,
    Query(query): Query<InspectQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let name = raw_name
        .parse::<ToolName>()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let response = state
        .registry
        .inspect_tool(&name, query.revision)
        .await
        .map_err(ApiError::from_storage)?;
    Ok(Json(response))
}

#[derive(Debug, Default, Deserialize)]
struct PageQuery {
    limit: Option<u32>,
    offset: Option<u64>,
}

async fn list_tool_versions(
    State(state): State<AppState>,
    Path(raw_name): Path<String>,
    Query(query): Query<PageQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let name = raw_name
        .parse::<ToolName>()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let (limit, offset) = validated_page(query.limit, query.offset)?;
    let response = state
        .registry
        .list_tool_versions(&name, limit, offset)
        .await
        .map_err(ApiError::from_storage)?;
    Ok(Json(response))
}

async fn get_tool_artifact(
    State(state): State<AppState>,
    Path((raw_name, revision)): Path<(String, u64)>,
) -> Result<impl IntoResponse, ApiError> {
    let name = raw_name
        .parse::<ToolName>()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    if revision == 0 {
        return Err(ApiError::bad_request("tool revision must be positive"));
    }
    let digest = state
        .registry
        .artifact_digest_for_version(&name, revision)
        .await
        .map_err(ApiError::from_storage)?;
    let store = state.artifact_store.clone();
    let lookup_digest = digest.clone();
    let artifact = tokio::task::spawn_blocking(move || store.load(&lookup_digest))
        .await
        .map_err(|error| ApiError::internal(format!("artifact task failed: {error}")))?
        .map_err(ApiError::from_artifact)?;
    let bundle = artifact.bundle;
    Ok(Json(ToolArtifactResponse {
        tool: name.to_string(),
        revision,
        digest,
        manifest: ToolArtifactManifest {
            format_version: bundle.manifest.format_version,
            runtime: bundle.manifest.runtime,
            input_format: bundle.manifest.input_format,
            output_format: bundle.manifest.output_format,
            source_sha256: bundle.manifest.source_sha256,
            http_capabilities: bundle.manifest.http_capabilities,
        },
        source: bundle.source,
        tests: bundle
            .tests
            .into_iter()
            .map(|test| ToolArtifactTestCase {
                name: test.name,
                args: test.args,
                stdin: test.stdin,
                expected_stdout: test.expected_stdout,
                expected_exit_code: test.expected_exit_code,
            })
            .collect(),
    }))
}

fn validated_page(limit: Option<u32>, offset: Option<u64>) -> Result<(u32, u64), ApiError> {
    let limit = limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(ApiError::bad_request("limit must be between 1 and 100"));
    }
    let offset = offset.unwrap_or(0);
    if offset > i64::MAX as u64 {
        return Err(ApiError::bad_request("offset is too large"));
    }
    Ok((limit, offset))
}

async fn invoke_tool(
    State(state): State<AppState>,
    Path(raw_name): Path<String>,
    Json(request): Json<InvocationRequest>,
) -> Result<Response, ApiError> {
    let name = raw_name
        .parse::<ToolName>()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    if request.args.len() > 128 || request.args.iter().any(|argument| argument.len() > 4096) {
        return Err(ApiError::bad_request("tool arguments exceed their limit"));
    }
    let timeout_ms = request.timeout_ms.unwrap_or(5_000);
    if !(1..=30_000).contains(&timeout_ms) {
        return Err(ApiError::bad_request(
            "timeout_ms must be between 1 and 30000",
        ));
    }
    let stdin = BASE64
        .decode(&request.stdin_base64)
        .map_err(|_| ApiError::bad_request("stdin_base64 is not valid Base64"))?;
    if stdin.len() > 4 * 1024 * 1024 {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "input_too_large",
            "call input exceeds 4 MiB",
        ));
    }

    let resolved = state
        .registry
        .resolve_tool(&name, request.revision)
        .await
        .map_err(ApiError::from_storage)?;
    let media_type = request
        .content_type
        .split(';')
        .next()
        .map(str::trim)
        .unwrap_or_default();
    let expected_media_type = match resolved.input_format {
        jit_protocol::IoFormat::Text => "text/plain",
        jit_protocol::IoFormat::Json => "application/json",
    };
    if media_type != expected_media_type {
        return Err(ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            format!("tool input requires {expected_media_type}"),
        ));
    }
    let invocation_id = Uuid::now_v7();
    state
        .registry
        .start_invocation(invocation_id, &resolved, stdin.len() as u64)
        .await
        .map_err(ApiError::from_storage)?;

    let mut grpc_request = GrpcRequest::new(ExecuteRequest {
        invocation_id: invocation_id.to_string(),
        artifact_digest: resolved.artifact_digest,
        args: request.args,
        stdin,
        timeout_ms,
    });
    let authorization = format!("Bearer {}", state.worker_token)
        .parse()
        .map_err(|_| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "invalid worker authentication configuration",
            )
        })?;
    grpc_request
        .metadata_mut()
        .insert("authorization", authorization);

    let output = match state.worker.clone().execute(grpc_request).await {
        Ok(response) => response.into_inner(),
        Err(status) => {
            let (invocation_status, http_status, code) = match status.code() {
                Code::DeadlineExceeded => (
                    "timed_out",
                    StatusCode::GATEWAY_TIMEOUT,
                    "execution_timeout",
                ),
                Code::ResourceExhausted => (
                    "failed",
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "output_limit_exceeded",
                ),
                Code::Unavailable => (
                    "failed",
                    StatusCode::SERVICE_UNAVAILABLE,
                    "worker_unavailable",
                ),
                Code::NotFound => (
                    "failed",
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "artifact_unavailable",
                ),
                _ => ("failed", StatusCode::BAD_GATEWAY, "execution_failed"),
            };
            if let Err(error) = state
                .registry
                .finish_invocation(invocation_id, invocation_status, None, 0, 0, 0)
                .await
            {
                error!(%error, %invocation_id, "failed to record invocation failure");
            }
            return Err(ApiError::new(http_status, code, status.message()));
        }
    };

    let invocation_status = if output.exit_code == 0 {
        "succeeded"
    } else {
        "failed"
    };
    if let Err(error) = state
        .registry
        .finish_invocation(
            invocation_id,
            invocation_status,
            Some(output.exit_code),
            output.duration_ms,
            output.stdout.len(),
            output.stderr.len(),
        )
        .await
    {
        error!(%error, %invocation_id, "failed to finish invocation record");
    }
    Ok(Json(InvocationResponse {
        invocation_id: invocation_id.to_string(),
        resolved_revision: resolved.revision,
        exit_code: output.exit_code,
        stdout_base64: BASE64.encode(output.stdout),
        stderr_base64: BASE64.encode(output.stderr),
        duration_ms: output.duration_ms,
    })
    .into_response())
}

async fn require_auth(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let supplied = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let bearer_authorized = supplied
        .map(|token| constant_time_eq(token.as_bytes(), state.auth_token.as_bytes()))
        .unwrap_or(false);
    if bearer_authorized {
        return next.run(request).await;
    }
    if let Some(session) = web::session_from_headers(&state.sessions, request.headers()) {
        let safe_method = matches!(
            *request.method(),
            Method::GET | Method::HEAD | Method::OPTIONS
        );
        if safe_method || web::csrf_matches(&session, request.headers()) {
            return next.run(request).await;
        }
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "csrf_failed",
            "a valid CSRF token is required",
        )
        .into_response();
    }
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "a valid bearer token or browser session is required",
    )
    .into_response()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && bool::from(left.ct_eq(right))
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    body: ErrorResponse,
}

impl ApiError {
    fn new(status: StatusCode, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status,
            body: ErrorResponse::new(code, message, format!("req_{}", Ulid::new())),
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
    }

    fn from_storage(error: StorageError) -> Self {
        match error {
            StorageError::ToolNotFound => {
                Self::new(StatusCode::NOT_FOUND, "tool_not_found", "tool not found")
            }
            StorageError::VersionNotFound => Self::new(
                StatusCode::NOT_FOUND,
                "version_not_found",
                "tool version not found",
            ),
            StorageError::VersionNotRevocable(status) => Self::new(
                StatusCode::CONFLICT,
                "version_not_revocable",
                format!("tool version in status {status:?} cannot be revoked"),
            ),
            StorageError::JobNotFound => Self::new(
                StatusCode::NOT_FOUND,
                "job_not_found",
                "synthesis job not found",
            ),
            StorageError::JobNotCancellable(status) => Self::new(
                StatusCode::CONFLICT,
                "job_not_cancellable",
                format!("synthesis job in status {status:?} cannot be cancelled"),
            ),
            StorageError::InvalidCancellationReason => Self::new(
                StatusCode::BAD_REQUEST,
                "invalid_cancellation_reason",
                error.to_string(),
            ),
            StorageError::JobInputNotFound => Self::new(
                StatusCode::CONFLICT,
                "job_input_not_pending",
                "the synthesis job no longer has that pending input",
            ),
            StorageError::InvalidJobInputAnswer => Self::new(
                StatusCode::BAD_REQUEST,
                "invalid_job_answer",
                "the answer does not match the pending input",
            ),
            StorageError::HttpCapabilityNotFound => Self::new(
                StatusCode::CONFLICT,
                "http_capability_not_active",
                "HTTP capability approval was not found or is already revoked",
            ),
            StorageError::InvalidRevocationReason => Self::new(
                StatusCode::BAD_REQUEST,
                "invalid_revocation_reason",
                error.to_string(),
            ),
            StorageError::IdempotencyConflict => Self::new(
                StatusCode::CONFLICT,
                "idempotency_conflict",
                error.to_string(),
            ),
            StorageError::ToolNotReady => Self::new(
                StatusCode::CONFLICT,
                "tool_not_ready",
                "the selected tool version has not completed synthesis and validation",
            ),
            StorageError::ArtifactNotFound => Self::new(
                StatusCode::NOT_FOUND,
                "artifact_not_found",
                "tool version has no published artifact",
            ),
            other => {
                error!(error = %other, "registry operation failed");
                Self::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "the registry operation failed",
                )
            }
        }
    }

    fn from_artifact(error: ArtifactError) -> Self {
        match error {
            ArtifactError::Io(ref source) if source.kind() == std::io::ErrorKind::NotFound => {
                Self::new(
                    StatusCode::NOT_FOUND,
                    "artifact_not_found",
                    "published artifact is not available",
                )
            }
            other => {
                error!(error = %other, "artifact lookup failed");
                Self::internal("failed to load the published artifact")
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.status, Json(self.body)).into_response();
        response.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/json"),
        );
        response
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("jit_server=info,tower_http=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

async fn shutdown_signal() {
    if let Err(error) = signal::ctrl_c().await {
        warn!(%error, "failed to install Ctrl+C handler");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_comparison_checks_length_and_content() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secrex"));
        assert!(!constant_time_eq(b"secret", b"secret-long"));
    }

    #[test]
    fn error_responses_have_machine_code_and_request_id() {
        let error = ApiError::bad_request("bad");
        assert_eq!(error.body.code, "invalid_request");
        assert!(error.body.request_id.starts_with("req_"));
        assert_eq!(error.body.details, None);
    }

    #[test]
    fn pagination_limits_are_bounded() {
        assert_eq!(validated_page(None, None).unwrap(), (50, 0));
        assert!(validated_page(Some(0), None).is_err());
        assert!(validated_page(Some(101), None).is_err());
        assert!(validated_page(Some(50), Some(i64::MAX as u64 + 1)).is_err());
    }
}
