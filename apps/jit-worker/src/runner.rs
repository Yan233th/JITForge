use std::{collections::HashSet, io, process::Stdio, time::Duration};

use jit_artifact::{ArtifactError, ArtifactStore, source_image_tag};
use jit_protocol::HttpFixture;
use jit_storage::Registry;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    time::{Instant, timeout},
};
use tracing::{info, warn};
use url::Url;
use uuid::Uuid;

const MAX_STREAM_BYTES: usize = 1024 * 1024;
const MAX_BUILD_TIME: Duration = Duration::from_secs(60);
const SANDBOX_RUN_OPTIONS: &[&str] = &[
    "--rm",
    "--user=65532:65532",
    "--read-only",
    "--cap-drop=ALL",
    "--security-opt=no-new-privileges",
    "--ulimit=nproc=16:16",
    "--ulimit=nofile=64:64",
    "--memory=128m",
    "--memory-swap=128m",
    "--cpus=0.5",
    "--tmpfs=/tmp:rw,noexec,nosuid,nodev,size=32m",
    "--env=PYTHONDONTWRITEBYTECODE=1",
    "--interactive",
];

#[derive(Clone)]
pub struct DockerRunner {
    store: ArtifactStore,
    runtime: String,
    docker_binary: String,
    http_mode: HttpMode,
    http_proxy_url: Option<String>,
    registry: Registry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HttpMode {
    Disabled,
    Direct,
}

#[derive(Clone, Debug)]
pub struct ExecutionOutput {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub duration_ms: u64,
}

impl DockerRunner {
    pub fn new(
        store: ArtifactStore,
        registry: Registry,
        runtime: impl Into<String>,
        http_mode: &str,
        http_proxy_url: Option<&str>,
    ) -> Result<Self, RunnerError> {
        let http_mode = match http_mode.trim() {
            "disabled" => HttpMode::Disabled,
            "direct" => HttpMode::Direct,
            other => return Err(RunnerError::InvalidHttpMode(other.to_owned())),
        };
        let http_proxy_url = normalize_proxy_url(http_proxy_url)?;
        Ok(Self {
            store,
            registry,
            runtime: runtime.into(),
            docker_binary: "docker".to_owned(),
            http_mode,
            http_proxy_url,
        })
    }

    pub async fn ensure_ready(&self) -> Result<(), RunnerError> {
        let output = Command::new(&self.docker_binary)
            .args(["info", "--format", "{{json .Runtimes}}"])
            .output()
            .await?;
        if !output.status.success() {
            return Err(RunnerError::Docker(
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ));
        }
        let runtimes = String::from_utf8_lossy(&output.stdout);
        if !runtimes.contains(&format!("\"{}\"", self.runtime)) {
            return Err(RunnerError::RuntimeUnavailable(self.runtime.clone()));
        }
        Ok(())
    }

    pub async fn build_artifact(&self, digest: &str) -> Result<String, RunnerError> {
        let stored = self.store.load(digest)?;
        let source_sha256 = &stored.bundle.manifest.source_sha256;
        let tag = source_image_tag(&stored.bundle.manifest.runtime, source_sha256)?;
        let present = Command::new(&self.docker_binary)
            .args([
                "image",
                "inspect",
                "--format",
                r#"{{ index .Config.Labels "dev.jitforge.source" }}"#,
                &tag,
            ])
            .output()
            .await?;
        if present.status.success() {
            let actual_source = normalize_label(&present.stdout);
            if actual_source != source_sha256 {
                return Err(RunnerError::ImageIntegrity {
                    expected: source_sha256.to_owned(),
                    actual: actual_source.to_owned(),
                });
            }
            return Ok(tag);
        }

        info!(%digest, %source_sha256, %tag, "building source image");
        let mut command = Command::new(&self.docker_binary);
        command
            .arg("build")
            .arg("--network=none")
            .arg("--tag")
            .arg(&tag)
            .arg("--label")
            .arg(format!("dev.jitforge.source={source_sha256}"))
            .arg(&stored.directory)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let output = timeout(MAX_BUILD_TIME, command.output())
            .await
            .map_err(|_| RunnerError::BuildTimeout)??;
        if !output.status.success() {
            return Err(RunnerError::BuildFailed(truncate_diagnostic(
                &output.stderr,
            )));
        }
        Ok(tag)
    }

    pub async fn remove_source_image(&self, source_sha256: &str) -> Result<(), RunnerError> {
        for runtime in ["python-stdlib-v1", "python-stdlib-v2"] {
            let tag = source_image_tag(runtime, source_sha256)?;
            let output = Command::new(&self.docker_binary)
                .args(["image", "rm", "--force", &tag])
                .output()
                .await?;
            if !output.status.success()
                && !String::from_utf8_lossy(&output.stderr).contains("No such image")
            {
                return Err(RunnerError::Docker(truncate_diagnostic(&output.stderr)));
            }
        }
        Ok(())
    }

    pub async fn cleanup_unreferenced_images(
        &self,
        referenced_sources: &HashSet<String>,
    ) -> Result<usize, RunnerError> {
        let output = Command::new(&self.docker_binary)
            .args([
                "image",
                "ls",
                "--filter",
                "label=dev.jitforge.source",
                "--format",
                "{{.Repository}}:{{.Tag}}",
            ])
            .output()
            .await?;
        if !output.status.success() {
            return Err(RunnerError::Docker(truncate_diagnostic(&output.stderr)));
        }
        let mut removed = 0;
        for tag in String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|tag| !tag.trim().is_empty())
        {
            let inspect = Command::new(&self.docker_binary)
                .args([
                    "image",
                    "inspect",
                    "--format",
                    r#"{{ index .Config.Labels "dev.jitforge.source" }}"#,
                    tag,
                ])
                .output()
                .await?;
            if !inspect.status.success() {
                continue;
            }
            let source = normalize_label(&inspect.stdout);
            if !referenced_sources.contains(source) {
                let removal = Command::new(&self.docker_binary)
                    .args(["image", "rm", "--force", tag])
                    .output()
                    .await?;
                if removal.status.success() {
                    removed += 1;
                }
            }
        }
        Ok(removed)
    }

    pub async fn execute(
        &self,
        digest: &str,
        args: &[String],
        stdin: &[u8],
        time_limit: Duration,
    ) -> Result<ExecutionOutput, RunnerError> {
        self.execute_inner(digest, args, stdin, time_limit, None)
            .await
    }

    pub async fn execute_with_fixtures(
        &self,
        digest: &str,
        args: &[String],
        stdin: &[u8],
        time_limit: Duration,
        fixtures: &[HttpFixture],
    ) -> Result<ExecutionOutput, RunnerError> {
        self.execute_inner(digest, args, stdin, time_limit, Some(fixtures))
            .await
    }

    async fn execute_inner(
        &self,
        digest: &str,
        args: &[String],
        stdin: &[u8],
        time_limit: Duration,
        fixtures: Option<&[HttpFixture]>,
    ) -> Result<ExecutionOutput, RunnerError> {
        let stored = self.store.load(digest)?;
        let has_http = !stored.bundle.manifest.http_capabilities.is_empty();
        if has_http {
            let hashes = stored
                .bundle
                .manifest
                .http_capabilities
                .iter()
                .map(|grant| grant.approval_hash.clone())
                .collect::<Vec<_>>();
            if !self
                .registry
                .all_http_capabilities_approved(&hashes)
                .await?
            {
                return Err(RunnerError::HttpApprovalRevoked);
            }
        }
        let fixture_json = fixtures
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| RunnerError::HttpFixture(error.to_string()))?;
        if fixture_json
            .as_ref()
            .is_some_and(|encoded| encoded.len() > 96 * 1024)
        {
            return Err(RunnerError::HttpFixture(
                "encoded HTTP fixtures exceed 96 KiB".to_owned(),
            ));
        }
        let network = if fixtures.is_some() {
            "none"
        } else if has_http {
            match self.http_mode {
                HttpMode::Direct => "bridge",
                HttpMode::Disabled => return Err(RunnerError::HttpDisabled),
            }
        } else {
            "none"
        };
        let image = self.build_artifact(digest).await?;
        let container_name = format!("jitforge-{}", Uuid::now_v7().simple());
        let started_at = Instant::now();
        let mut command = Command::new(&self.docker_binary);
        command
            .arg("run")
            .arg("--name")
            .arg(&container_name)
            .arg("--runtime")
            .arg(&self.runtime)
            .arg("--network")
            .arg(network)
            .args(SANDBOX_RUN_OPTIONS)
            .arg("--env")
            .arg(if fixtures.is_some() {
                "JITFORGE_HTTP_MODE=fixture"
            } else if has_http {
                "JITFORGE_HTTP_MODE=direct"
            } else {
                "JITFORGE_HTTP_MODE=disabled"
            });
        if let Some(fixtures) = fixture_json {
            command
                .arg("--env")
                .arg(format!("JITFORGE_HTTP_FIXTURES={fixtures}"));
        }
        if fixtures.is_none()
            && has_http
            && let Some(proxy_url) = &self.http_proxy_url
        {
            command
                .arg("--env")
                .arg(format!("HTTPS_PROXY={proxy_url}"))
                .arg("--env")
                .arg(format!("https_proxy={proxy_url}"));
        }
        command
            .arg(&image)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command.spawn()?;
        let mut child_stdin = child.stdin.take().ok_or(RunnerError::MissingPipe)?;
        let child_stdout = child.stdout.take().ok_or(RunnerError::MissingPipe)?;
        let child_stderr = child.stderr.take().ok_or(RunnerError::MissingPipe)?;
        let input = stdin.to_vec();
        let input_task = tokio::spawn(async move {
            child_stdin.write_all(&input).await?;
            child_stdin.shutdown().await
        });
        let mut output_task = tokio::spawn(async move {
            tokio::try_join!(
                read_bounded(child_stdout, MAX_STREAM_BYTES),
                read_bounded(child_stderr, MAX_STREAM_BYTES)
            )
        });

        let mut completed_output = None;
        let status = tokio::select! {
            status = child.wait() => status?,
            output = &mut output_task => {
                match output {
                    Ok(Ok(output)) => {
                        completed_output = Some(output);
                        child.wait().await?
                    },
                    Ok(Err(error)) if error.kind() == io::ErrorKind::FileTooLarge => {
                        terminate(&mut child, &container_name, &self.docker_binary).await;
                        return Err(RunnerError::OutputLimitExceeded);
                    }
                    Ok(Err(error)) => {
                        terminate(&mut child, &container_name, &self.docker_binary).await;
                        return Err(RunnerError::Io(error));
                    }
                    Err(error) => {
                        terminate(&mut child, &container_name, &self.docker_binary).await;
                        return Err(RunnerError::Task(error.to_string()));
                    }
                }
            }
            _ = tokio::time::sleep(time_limit) => {
                terminate(&mut child, &container_name, &self.docker_binary).await;
                return Err(RunnerError::Timeout);
            }
        };

        if let Err(error) = input_task
            .await
            .map_err(|error| RunnerError::Task(error.to_string()))?
            && error.kind() != io::ErrorKind::BrokenPipe
        {
            return Err(RunnerError::Io(error));
        }
        let (stdout, stderr) = match completed_output {
            Some(output) => output,
            None => output_task
                .await
                .map_err(|error| RunnerError::Task(error.to_string()))??,
        };
        let exit_code = status
            .code()
            .ok_or_else(|| RunnerError::Docker("docker process terminated by signal".to_owned()))?;
        if exit_code >= 125 {
            return Err(RunnerError::Docker(truncate_diagnostic(&stderr)));
        }
        Ok(ExecutionOutput {
            exit_code,
            stdout,
            stderr,
            duration_ms: started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        })
    }
}

fn normalize_proxy_url(proxy_url: Option<&str>) -> Result<Option<String>, RunnerError> {
    let Some(raw_url) = proxy_url.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let url =
        Url::parse(raw_url).map_err(|error| RunnerError::InvalidHttpProxy(error.to_string()))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(RunnerError::InvalidHttpProxy(
            "proxy URL must be an HTTP(S) origin without credentials, path, query, or fragment"
                .to_owned(),
        ));
    }
    Ok(Some(url.to_string()))
}

fn normalize_label(output: &[u8]) -> &str {
    std::str::from_utf8(output).unwrap_or_default().trim()
}

async fn read_bounded<R>(mut reader: R, limit: usize) -> io::Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut output = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(output);
        }
        if output.len().saturating_add(read) > limit {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "runner output limit exceeded",
            ));
        }
        output.extend_from_slice(&buffer[..read]);
    }
}

async fn terminate(child: &mut Child, container_name: &str, docker_binary: &str) {
    let _ = child.kill().await;
    let result = Command::new(docker_binary)
        .args(["rm", "--force", container_name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
    if let Err(error) = result {
        warn!(%error, %container_name, "failed to clean up runner container");
    }
}

fn truncate_diagnostic(bytes: &[u8]) -> String {
    const LIMIT: usize = 8 * 1024;
    String::from_utf8_lossy(&bytes[..bytes.len().min(LIMIT)]).into_owned()
}

#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error("Docker runtime {0:?} is unavailable")]
    RuntimeUnavailable(String),

    #[error("unsupported runner HTTP mode {0:?}")]
    InvalidHttpMode(String),

    #[error("invalid runner HTTP proxy: {0}")]
    InvalidHttpProxy(String),

    #[error("artifact requires live HTTP, but runner HTTP mode is disabled")]
    HttpDisabled,

    #[error("artifact HTTP capability approval is missing or revoked")]
    HttpApprovalRevoked,

    #[error("invalid HTTP fixture configuration: {0}")]
    HttpFixture(String),

    #[error("Docker operation failed: {0}")]
    Docker(String),

    #[error("artifact build failed: {0}")]
    BuildFailed(String),

    #[error("artifact build timed out")]
    BuildTimeout,

    #[error("artifact image digest label mismatch: expected {expected}, got {actual:?}")]
    ImageIntegrity { expected: String, actual: String },

    #[error("tool execution timed out")]
    Timeout,

    #[error("tool output exceeded its limit")]
    OutputLimitExceeded,

    #[error("runner child process was missing a standard stream")]
    MissingPipe,

    #[error("runner task failed: {0}")]
    Task(String),

    #[error(transparent)]
    Artifact(#[from] ArtifactError),

    #[error(transparent)]
    Storage(#[from] jit_storage::StorageError),

    #[error(transparent)]
    Io(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn bounded_reader_rejects_excess_output() {
        let (mut writer, reader) = tokio::io::duplex(32);
        let task = tokio::spawn(async move {
            writer.write_all(b"123456").await.unwrap();
        });
        let error = read_bounded(reader, 5).await.unwrap_err();
        task.await.unwrap();
        assert_eq!(error.kind(), io::ErrorKind::FileTooLarge);
    }

    #[test]
    fn cached_image_labels_are_normalized_before_comparison() {
        assert_eq!(normalize_label(b"sha256:abc\n"), "sha256:abc");
        assert_eq!(normalize_label(&[0xff]), "");
    }

    #[test]
    fn sandbox_options_pin_process_and_identity_limits() {
        for required in [
            "--user=65532:65532",
            "--ulimit=nproc=16:16",
            "--memory-swap=128m",
            "--ulimit=nofile=64:64",
        ] {
            assert!(SANDBOX_RUN_OPTIONS.contains(&required));
        }
        assert!(!SANDBOX_RUN_OPTIONS.contains(&"--network=bridge"));
    }
}
