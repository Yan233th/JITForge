use std::{collections::HashSet, io, process::Stdio, time::Duration};

use jit_artifact::{ArtifactError, ArtifactStore, source_image_tag};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    time::{Instant, timeout},
};
use tracing::{info, warn};
use uuid::Uuid;

const MAX_STREAM_BYTES: usize = 1024 * 1024;
const MAX_BUILD_TIME: Duration = Duration::from_secs(60);
const SANDBOX_RUN_OPTIONS: &[&str] = &[
    "--rm",
    "--network=none",
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
}

#[derive(Clone, Debug)]
pub struct ExecutionOutput {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub duration_ms: u64,
}

impl DockerRunner {
    pub fn new(store: ArtifactStore, runtime: impl Into<String>) -> Self {
        Self {
            store,
            runtime: runtime.into(),
            docker_binary: "docker".to_owned(),
        }
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
        let tag = source_image_tag(source_sha256)?;
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
        let tag = source_image_tag(source_sha256)?;
        let output = Command::new(&self.docker_binary)
            .args(["image", "rm", "--force", &tag])
            .output()
            .await?;
        if !output.status.success()
            && !String::from_utf8_lossy(&output.stderr).contains("No such image")
        {
            return Err(RunnerError::Docker(truncate_diagnostic(&output.stderr)));
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
            .args(SANDBOX_RUN_OPTIONS)
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
            "--network=none",
            "--user=65532:65532",
            "--ulimit=nproc=16:16",
            "--memory-swap=128m",
            "--ulimit=nofile=64:64",
        ] {
            assert!(SANDBOX_RUN_OPTIONS.contains(&required));
        }
    }
}
