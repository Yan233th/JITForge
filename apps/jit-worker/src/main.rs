mod jobs;
mod runner;
mod service;
mod synthesizer;

use std::{env, net::SocketAddr, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use jit_artifact::ArtifactStore;
use jit_protocol::worker::runner_server::RunnerServer;
use jit_storage::Registry;
use runner::DockerRunner;
use service::GrpcRunnerService;
use synthesizer::{FixtureSynthesizer, OpenAiSynthesizer, Synthesizer};
use tokio::{signal, time::sleep};
use tonic::transport::Server;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

const DEFAULT_DATABASE_URL: &str = "postgres://jitforge:jitforge@127.0.0.1:5432/jitforge";
const DEFAULT_LISTEN_ADDR: &str = "127.0.0.1:50051";
const DEFAULT_ARTIFACT_DIR: &str = ".data/artifacts";

#[derive(Debug)]
struct Config {
    database_url: String,
    listen_addr: SocketAddr,
    artifact_dir: String,
    worker_token: String,
    worker_id: String,
    docker_runtime: String,
    synthesizer_mode: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let config = Config::from_env()?;
    let registry = Registry::connect(&config.database_url, 10)
        .await
        .context("failed to connect to PostgreSQL")?;
    registry.migrate().await?;
    let artifact_store = ArtifactStore::new(&config.artifact_dir);
    let runner = Arc::new(DockerRunner::new(
        artifact_store.clone(),
        &config.docker_runtime,
    ));
    runner
        .ensure_ready()
        .await
        .context("runner prerequisite check failed")?;
    cleanup_stale_state(&registry, &artifact_store, &runner).await;

    let heartbeat_registry = registry.clone();
    let heartbeat_worker_id = config.worker_id.clone();
    tokio::spawn(async move {
        loop {
            if let Err(error) = heartbeat_registry
                .record_worker_heartbeat(&heartbeat_worker_id, env!("CARGO_PKG_VERSION"))
                .await
            {
                error!(%error, "worker heartbeat failed");
            }
            sleep(Duration::from_secs(10)).await;
        }
    });

    let compaction_registry = registry.clone();
    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(60 * 60)).await;
            match compaction_registry.compact_agent_traces().await {
                Ok(count) if count > 0 => info!(count, "compacted old agent traces"),
                Ok(_) => {}
                Err(error) => warn!(%error, "agent trace compaction failed"),
            }
        }
    });

    if let Some(synthesizer) = build_synthesizer(&config.synthesizer_mode)? {
        let processor = jobs::JobProcessor::new(
            registry.clone(),
            artifact_store,
            runner.clone(),
            synthesizer,
            config.worker_id.clone(),
        );
        tokio::spawn(processor.run());
    } else {
        warn!("synthesis job processing is disabled");
    }

    let service = GrpcRunnerService::new(runner, config.worker_token);
    info!(listen_addr = %config.listen_addr, worker_id = %config.worker_id, "jit-worker listening");
    Server::builder()
        .add_service(RunnerServer::new(service))
        .serve_with_shutdown(config.listen_addr, shutdown_signal())
        .await
        .context("worker gRPC server failed")
}

async fn cleanup_stale_state(registry: &Registry, store: &ArtifactStore, runner: &DockerRunner) {
    match store.cleanup_temporary() {
        Ok(count) if count > 0 => info!(count, "removed interrupted artifact writes"),
        Ok(_) => {}
        Err(error) => warn!(%error, "failed to clean interrupted artifact writes"),
    }
    let (referenced_digests, referenced_sources) = match registry.referenced_artifacts().await {
        Ok(referenced) => referenced,
        Err(error) => {
            warn!(%error, "failed to load artifact references for cleanup");
            return;
        }
    };
    match store.list_digests() {
        Ok(digests) => {
            let mut removed = 0;
            for digest in digests {
                if !referenced_digests.contains(&digest) {
                    match store.remove(&digest) {
                        Ok(true) => removed += 1,
                        Ok(false) => {}
                        Err(error) => warn!(%digest, %error, "failed to remove stale artifact"),
                    }
                }
            }
            if removed > 0 {
                info!(removed, "removed unreferenced artifact directories");
            }
        }
        Err(error) => warn!(%error, "failed to list artifacts for cleanup"),
    }
    match runner
        .cleanup_unreferenced_images(&referenced_sources)
        .await
    {
        Ok(count) if count > 0 => info!(count, "removed unreferenced source images"),
        Ok(_) => {}
        Err(error) => warn!(%error, "failed to clean source images"),
    }
}

impl Config {
    fn from_env() -> Result<Self> {
        let worker_token =
            env::var("JITFORGE_WORKER_TOKEN").context("JITFORGE_WORKER_TOKEN is required")?;
        if worker_token.trim().is_empty() {
            bail!("JITFORGE_WORKER_TOKEN must not be empty");
        }
        Ok(Self {
            database_url: env::var("JITFORGE_DATABASE_URL")
                .unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_owned()),
            listen_addr: env::var("JITFORGE_WORKER_LISTEN_ADDR")
                .unwrap_or_else(|_| DEFAULT_LISTEN_ADDR.to_owned())
                .parse()
                .context("JITFORGE_WORKER_LISTEN_ADDR must be a socket address")?,
            artifact_dir: env::var("JITFORGE_ARTIFACT_DIR")
                .unwrap_or_else(|_| DEFAULT_ARTIFACT_DIR.to_owned()),
            worker_token,
            worker_id: env::var("JITFORGE_WORKER_ID")
                .unwrap_or_else(|_| format!("worker_{}", Uuid::now_v7())),
            docker_runtime: env::var("JITFORGE_DOCKER_RUNTIME")
                .unwrap_or_else(|_| "runsc".to_owned()),
            synthesizer_mode: env::var("JITFORGE_SYNTHESIZER_MODE")
                .unwrap_or_else(|_| "openai".to_owned()),
        })
    }
}

fn build_synthesizer(mode: &str) -> Result<Option<Arc<dyn Synthesizer>>> {
    match mode {
        "openai" => Ok(Some(Arc::new(OpenAiSynthesizer::from_env()?))),
        "fixture" => Ok(Some(Arc::new(FixtureSynthesizer))),
        "disabled" => Ok(None),
        other => bail!(
            "unsupported JITFORGE_SYNTHESIZER_MODE {other:?}; expected openai, fixture, or disabled"
        ),
    }
}

fn init_tracing() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("jit_worker=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

async fn shutdown_signal() {
    if let Err(error) = signal::ctrl_c().await {
        warn!(%error, "failed to install Ctrl+C handler");
    }
}
