mod jobs;
mod runner;
mod service;
mod synthesizer;
mod web_access;

use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use jit_artifact::ArtifactStore;
use jit_config::{JitForgeConfig, nonempty_env};
use jit_protocol::worker::runner_server::RunnerServer;
use jit_storage::Registry;
use runner::DockerRunner;
use service::GrpcRunnerService;
use synthesizer::{FixtureSynthesizer, Synthesizer, build_rig_synthesizer};
use tokio::{signal, time::sleep};
use tonic::transport::Server;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;
use web_access::WebAccess;

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
    llm: LlmSettings,
    search: SearchSettings,
    http_mode: String,
    http_proxy_url: Option<String>,
}

#[derive(Debug)]
struct LlmSettings {
    protocol: String,
    base_url: Option<String>,
    api_key: Option<String>,
    model: Option<String>,
    verifier_model: Option<String>,
    thinking: String,
}

#[derive(Debug)]
struct SearchSettings {
    provider: String,
    base_url: String,
    engines: String,
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
    let web_access = Arc::new(
        WebAccess::new(
            &config.search.provider,
            &config.search.base_url,
            &config.search.engines,
            config.http_proxy_url.as_deref(),
        )
        .context("failed to configure synthesis web access")?,
    );
    let runner = Arc::new(
        DockerRunner::new(
            artifact_store.clone(),
            registry.clone(),
            &config.docker_runtime,
            &config.http_mode,
            config.http_proxy_url.as_deref(),
        )
        .context("failed to configure runner HTTP mode")?,
    );
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

    if let Some(synthesizer) = build_synthesizer(&config.synthesizer_mode, &config.llm)? {
        let processor = jobs::JobProcessor::new(
            registry.clone(),
            artifact_store,
            runner.clone(),
            synthesizer,
            web_access,
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
        let config = JitForgeConfig::load(None).context("failed to load JITForge configuration")?;
        let worker_token = configured("JITFORGE_WORKER_TOKEN", config.auth.worker_token)
            .context("worker token is required in configuration or JITFORGE_WORKER_TOKEN")?;
        Ok(Self {
            database_url: configured("JITFORGE_DATABASE_URL", config.worker.database_url)
                .unwrap_or_else(|| DEFAULT_DATABASE_URL.to_owned()),
            listen_addr: configured("JITFORGE_WORKER_LISTEN_ADDR", config.worker.listen_addr)
                .unwrap_or_else(|| DEFAULT_LISTEN_ADDR.to_owned())
                .parse()
                .context("JITFORGE_WORKER_LISTEN_ADDR must be a socket address")?,
            artifact_dir: configured("JITFORGE_ARTIFACT_DIR", config.worker.artifact_dir)
                .unwrap_or_else(|| DEFAULT_ARTIFACT_DIR.to_owned()),
            worker_token,
            worker_id: configured("JITFORGE_WORKER_ID", config.worker.worker_id)
                .unwrap_or_else(|| format!("worker_{}", Uuid::now_v7())),
            docker_runtime: configured("JITFORGE_DOCKER_RUNTIME", config.worker.docker_runtime)
                .unwrap_or_else(|| "runsc".to_owned()),
            synthesizer_mode: configured(
                "JITFORGE_SYNTHESIZER_MODE",
                config.worker.synthesizer_mode,
            )
            .unwrap_or_else(|| "openai".to_owned()),
            llm: LlmSettings {
                protocol: configured("JITFORGE_LLM_PROTOCOL", config.llm.protocol)
                    .unwrap_or_else(|| "chat_completions".to_owned()),
                base_url: configured("JITFORGE_LLM_BASE_URL", config.llm.base_url),
                api_key: configured("JITFORGE_LLM_API_KEY", config.llm.api_key),
                model: configured("JITFORGE_LLM_MODEL", config.llm.model),
                verifier_model: configured(
                    "JITFORGE_LLM_VERIFIER_MODEL",
                    config.llm.verifier_model,
                ),
                thinking: configured("JITFORGE_LLM_THINKING", config.llm.thinking)
                    .unwrap_or_else(|| "auto".to_owned()),
            },
            search: SearchSettings {
                provider: configured("JITFORGE_SEARCH_PROVIDER", config.search.provider)
                    .unwrap_or_else(|| "searxng".to_owned()),
                base_url: configured("JITFORGE_SEARCH_BASE_URL", config.search.base_url)
                    .unwrap_or_else(|| "http://127.0.0.1:8888/".to_owned()),
                engines: configured("JITFORGE_SEARCH_ENGINES", config.search.engines)
                    .unwrap_or_else(|| "mojeek".to_owned()),
            },
            http_mode: configured("JITFORGE_HTTP_MODE", config.http.mode)
                .unwrap_or_else(|| "disabled".to_owned()),
            http_proxy_url: configured("JITFORGE_HTTP_PROXY_URL", config.http.proxy_url),
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

fn build_synthesizer(mode: &str, llm: &LlmSettings) -> Result<Option<Arc<dyn Synthesizer>>> {
    match mode {
        "openai" | "rig" => Ok(Some(build_rig_synthesizer(
            &llm.protocol,
            llm.base_url.clone(),
            llm.api_key.clone(),
            llm.model.clone(),
            llm.verifier_model.clone(),
            &llm.thinking,
        )?)),
        "fixture" => Ok(Some(Arc::new(FixtureSynthesizer))),
        "disabled" => Ok(None),
        other => bail!(
            "unsupported JITFORGE_SYNTHESIZER_MODE {other:?}; expected rig, openai, fixture, or disabled"
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
