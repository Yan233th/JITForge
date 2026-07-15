use std::{sync::Arc, time::Duration};

use jit_protocol::worker::{
    ExecuteRequest, ExecuteResponse, runner_server::Runner as RunnerService,
};
use subtle::ConstantTimeEq;
use tonic::{Request, Response, Status};

use crate::runner::{DockerRunner, RunnerError};

#[derive(Clone)]
pub struct GrpcRunnerService {
    runner: Arc<DockerRunner>,
    token: Arc<String>,
}

impl GrpcRunnerService {
    pub fn new(runner: Arc<DockerRunner>, token: String) -> Self {
        Self {
            runner,
            token: Arc::new(token),
        }
    }

    fn authenticated<T>(&self, request: &Request<T>) -> bool {
        let supplied = request
            .metadata()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "));
        supplied
            .map(|value| {
                value.len() == self.token.len()
                    && bool::from(value.as_bytes().ct_eq(self.token.as_bytes()))
            })
            .unwrap_or(false)
    }
}

#[tonic::async_trait]
impl RunnerService for GrpcRunnerService {
    async fn execute(
        &self,
        request: Request<ExecuteRequest>,
    ) -> Result<Response<ExecuteResponse>, Status> {
        if !self.authenticated(&request) {
            return Err(Status::unauthenticated("invalid worker token"));
        }
        let request = request.into_inner();
        if request.stdin.len() > 4 * 1024 * 1024 {
            return Err(Status::invalid_argument("stdin exceeds 4 MiB"));
        }
        if !(1..=30_000).contains(&request.timeout_ms) {
            return Err(Status::invalid_argument(
                "timeout_ms must be between 1 and 30000",
            ));
        }
        if request.args.len() > 128 || request.args.iter().any(|arg| arg.len() > 4096) {
            return Err(Status::invalid_argument(
                "tool arguments exceed their limit",
            ));
        }

        let output = self
            .runner
            .execute(
                &request.artifact_digest,
                &request.args,
                &request.stdin,
                Duration::from_millis(request.timeout_ms),
            )
            .await
            .map_err(map_runner_error)?;
        Ok(Response::new(ExecuteResponse {
            exit_code: output.exit_code,
            stdout: output.stdout,
            stderr: output.stderr,
            duration_ms: output.duration_ms,
        }))
    }
}

fn map_runner_error(error: RunnerError) -> Status {
    match error {
        RunnerError::Timeout => Status::deadline_exceeded(error.to_string()),
        RunnerError::OutputLimitExceeded => Status::resource_exhausted(error.to_string()),
        RunnerError::RuntimeUnavailable(_) | RunnerError::Docker(_) => {
            Status::unavailable(error.to_string())
        }
        RunnerError::Artifact(_) => Status::not_found(error.to_string()),
        _ => Status::internal(error.to_string()),
    }
}
