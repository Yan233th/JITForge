use std::{
    env, fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use thiserror::Error;

pub const CONFIG_ENV: &str = "JITFORGE_CONFIG";

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct JitForgeConfig {
    pub auth: AuthConfig,
    pub client: ClientConfig,
    pub server: ServerConfig,
    pub worker: WorkerConfig,
    pub llm: LlmConfig,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuthConfig {
    pub token: Option<String>,
    pub worker_token: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClientConfig {
    pub server: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub listen_addr: Option<String>,
    pub database_url: Option<String>,
    pub worker_endpoint: Option<String>,
    pub artifact_dir: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkerConfig {
    pub database_url: Option<String>,
    pub listen_addr: Option<String>,
    pub artifact_dir: Option<String>,
    pub worker_id: Option<String>,
    pub docker_runtime: Option<String>,
    pub synthesizer_mode: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LlmConfig {
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub verifier_model: Option<String>,
    pub thinking: Option<String>,
}

impl JitForgeConfig {
    pub fn load(explicit_path: Option<&Path>) -> Result<Self, ConfigError> {
        let (path, required) = match explicit_path {
            Some(path) => (path.to_owned(), true),
            None => match nonempty_env(CONFIG_ENV) {
                Some(path) => (PathBuf::from(path), true),
                None => match default_path() {
                    Some(path) => (path, false),
                    None => return Ok(Self::default()),
                },
            },
        };
        if !path.exists() && !required {
            return Ok(Self::default());
        }
        let encoded = fs::read_to_string(&path).map_err(|source| ConfigError::Read {
            path: path.clone(),
            source,
        })?;
        let config: Self = toml::from_str(&encoded).map_err(|source| ConfigError::Parse {
            path: path.clone(),
            source,
        })?;
        validate_secret_permissions(&path, &config)?;
        Ok(config)
    }
}

pub fn nonempty_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn default_path() -> Option<PathBuf> {
    if let Some(root) = nonempty_env("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(root).join("jitforge/config.toml"));
    }
    nonempty_env("HOME").map(|home| PathBuf::from(home).join(".config/jitforge/config.toml"))
}

#[cfg(unix)]
fn validate_secret_permissions(path: &Path, config: &JitForgeConfig) -> Result<(), ConfigError> {
    use std::os::unix::fs::PermissionsExt;

    let contains_secrets = config.auth.token.is_some()
        || config.auth.worker_token.is_some()
        || config.llm.api_key.is_some();
    if contains_secrets {
        let mode = fs::metadata(path)
            .map_err(|source| ConfigError::Read {
                path: path.to_owned(),
                source,
            })?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            return Err(ConfigError::InsecurePermissions(path.to_owned()));
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_secret_permissions(_path: &Path, _config: &JitForgeConfig) -> Result<(), ConfigError> {
    Ok(())
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read configuration {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to parse configuration {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },

    #[error(
        "configuration {0} contains secrets and must not be accessible by group or others; run chmod 600"
    )]
    InsecurePermissions(PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_config(contents: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            env::temp_dir().join(format!("jitforge-config-{}-{nonce}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("config.toml");
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn parses_sections_and_rejects_unknown_fields() {
        let config: JitForgeConfig = toml::from_str(
            r#"
            [auth]
            token = "client"

            [client]
            server = "http://localhost:8080"

            [server]
            artifact_dir = ".data/artifacts"

            [llm]
            model = "coder"
            "#,
        )
        .unwrap();
        assert_eq!(config.auth.token.as_deref(), Some("client"));
        assert_eq!(
            config.client.server.as_deref(),
            Some("http://localhost:8080")
        );
        assert_eq!(config.llm.model.as_deref(), Some("coder"));
        assert_eq!(
            config.server.artifact_dir.as_deref(),
            Some(".data/artifacts")
        );
        assert!(toml::from_str::<JitForgeConfig>("unknown = true").is_err());
    }

    #[test]
    fn loads_an_explicit_configuration_file() {
        let path = temporary_config(
            r#"
            [client]
            server = "http://localhost:8080"
            "#,
        );
        let config = JitForgeConfig::load(Some(&path)).unwrap();
        assert_eq!(
            config.client.server.as_deref(),
            Some("http://localhost:8080")
        );
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn secret_configuration_requires_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let path = temporary_config("[llm]\napi_key = \"secret\"\n");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            JitForgeConfig::load(Some(&path)),
            Err(ConfigError::InsecurePermissions(_))
        ));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
