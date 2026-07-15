use std::{
    fs, io,
    path::{Path, PathBuf},
};

use jit_protocol::IoFormat;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

pub const RUNTIME_IMAGE: &str = "jitforge/python-stdlib-v1:0.1.0";

const ARTIFACT_DOCKERFILE: &str = r#"FROM jitforge/python-stdlib-v1:0.1.0
COPY --chown=65532:65532 tool.py /opt/jitforge/tool.py
USER 65532:65532
ENTRYPOINT ["python3", "-I", "-S", "-B", "/opt/jitforge/tool.py"]
"#;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolContract {
    pub summary: String,
    #[serde(default)]
    pub assumptions: Vec<String>,
    #[serde(default)]
    pub invariants: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolTestCase {
    pub name: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub stdin: String,
    pub expected_stdout: String,
    #[serde(default)]
    pub expected_exit_code: i32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ValidationSummary {
    pub tests_total: usize,
    pub tests_passed: usize,
    pub repair_rounds: u32,
    #[serde(default)]
    pub agent_turns: u32,
    #[serde(default)]
    pub generated_test_corrections: u32,
    #[serde(default)]
    pub probes_run: u32,
    pub validated_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ArtifactManifest {
    pub format_version: u32,
    pub runtime: String,
    pub input_format: IoFormat,
    pub output_format: IoFormat,
    pub source_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ArtifactBundle {
    pub manifest: ArtifactManifest,
    pub contract: ToolContract,
    pub source: String,
    pub tests: Vec<ToolTestCase>,
    pub validation: ValidationSummary,
}

#[derive(Serialize)]
struct ArtifactDigestPayload<'a> {
    manifest: &'a ArtifactManifest,
    contract: &'a ToolContract,
    source: &'a str,
    tests: &'a [ToolTestCase],
    validation: ValidationDigestPayload,
}

#[derive(Serialize)]
struct ValidationDigestPayload {
    tests_total: usize,
    tests_passed: usize,
    repair_rounds: u32,
    agent_turns: u32,
    generated_test_corrections: u32,
    probes_run: u32,
}

#[derive(Clone, Debug)]
pub struct StoredArtifact {
    pub digest: String,
    pub relative_path: String,
    pub directory: PathBuf,
    pub size_bytes: u64,
    pub bundle: ArtifactBundle,
}

#[derive(Clone, Debug)]
pub struct ArtifactStore {
    root: PathBuf,
}

impl ArtifactStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn put(
        &self,
        input_format: IoFormat,
        output_format: IoFormat,
        contract: ToolContract,
        source: String,
        tests: Vec<ToolTestCase>,
        validation: ValidationSummary,
    ) -> Result<StoredArtifact, ArtifactError> {
        validate_source(&source)?;
        let source_sha256 = hex::encode(Sha256::digest(source.as_bytes()));
        let bundle = ArtifactBundle {
            manifest: ArtifactManifest {
                format_version: 2,
                runtime: "python-stdlib-v1".to_owned(),
                input_format,
                output_format,
                source_sha256,
            },
            contract,
            source,
            tests,
            validation,
        };
        let encoded = serde_json::to_vec(&bundle)?;
        let hex_digest = semantic_digest(&bundle)?;
        let digest = format!("sha256:{hex_digest}");
        let relative_path = format!("sha256/{}/{hex_digest}", &hex_digest[..2]);
        let directory = self.root.join(&relative_path);

        if directory.exists() {
            return self.load(&digest);
        }

        let parent = directory
            .parent()
            .ok_or_else(|| ArtifactError::InvalidDigest(digest.clone()))?;
        fs::create_dir_all(parent)?;
        let temporary = parent.join(format!(".tmp-{}", Uuid::now_v7()));
        fs::create_dir(&temporary)?;
        let write_result = (|| -> Result<(), ArtifactError> {
            fs::write(temporary.join("bundle.json"), &encoded)?;
            fs::write(
                temporary.join("manifest.json"),
                serde_json::to_vec_pretty(&bundle.manifest)?,
            )?;
            fs::write(
                temporary.join("contract.json"),
                serde_json::to_vec_pretty(&bundle.contract)?,
            )?;
            fs::write(
                temporary.join("tests.json"),
                serde_json::to_vec_pretty(&bundle.tests)?,
            )?;
            fs::write(
                temporary.join("validation.json"),
                serde_json::to_vec_pretty(&bundle.validation)?,
            )?;
            fs::write(temporary.join("tool.py"), &bundle.source)?;
            fs::write(temporary.join("Dockerfile"), ARTIFACT_DOCKERFILE)?;
            fs::rename(&temporary, &directory)?;
            Ok(())
        })();
        if write_result.is_err() {
            let _ = fs::remove_dir_all(&temporary);
        }
        write_result?;

        Ok(StoredArtifact {
            digest,
            relative_path,
            directory,
            size_bytes: encoded.len() as u64,
            bundle,
        })
    }

    pub fn load(&self, digest: &str) -> Result<StoredArtifact, ArtifactError> {
        let hex_digest = digest
            .strip_prefix("sha256:")
            .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or_else(|| ArtifactError::InvalidDigest(digest.to_owned()))?;
        let relative_path = format!("sha256/{}/{hex_digest}", &hex_digest[..2]);
        let directory = self.root.join(&relative_path);
        let encoded = fs::read(directory.join("bundle.json"))?;
        let bundle: ArtifactBundle = serde_json::from_slice(&encoded)?;
        let actual = match bundle.manifest.format_version {
            1 => hex::encode(Sha256::digest(&encoded)),
            2 => semantic_digest(&bundle)?,
            version => return Err(ArtifactError::UnsupportedFormat(version)),
        };
        if actual != hex_digest {
            return Err(ArtifactError::DigestMismatch {
                expected: hex_digest.to_owned(),
                actual,
            });
        }
        Ok(StoredArtifact {
            digest: digest.to_owned(),
            relative_path,
            directory,
            size_bytes: encoded.len() as u64,
            bundle,
        })
    }

    pub fn remove(&self, digest: &str) -> Result<bool, ArtifactError> {
        let directory = self.path_for_digest(digest)?;
        if !directory.exists() {
            return Ok(false);
        }
        fs::remove_dir_all(directory)?;
        Ok(true)
    }

    pub fn list_digests(&self) -> Result<Vec<String>, ArtifactError> {
        let mut digests = Vec::new();
        let base = self.root.join("sha256");
        if !base.exists() {
            return Ok(digests);
        }
        for prefix in fs::read_dir(base)? {
            let prefix = prefix?;
            if !prefix.file_type()?.is_dir() {
                continue;
            }
            for entry in fs::read_dir(prefix.path())? {
                let entry = entry?;
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if entry.file_type()?.is_dir()
                    && name.len() == 64
                    && name.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    digests.push(format!("sha256:{name}"));
                }
            }
        }
        Ok(digests)
    }

    pub fn cleanup_temporary(&self) -> Result<usize, ArtifactError> {
        let mut removed = 0;
        let base = self.root.join("sha256");
        if !base.exists() {
            return Ok(removed);
        }
        for prefix in fs::read_dir(base)? {
            let prefix = prefix?;
            if !prefix.file_type()?.is_dir() {
                continue;
            }
            for entry in fs::read_dir(prefix.path())? {
                let entry = entry?;
                if entry.file_type()?.is_dir()
                    && entry.file_name().to_string_lossy().starts_with(".tmp-")
                {
                    fs::remove_dir_all(entry.path())?;
                    removed += 1;
                }
            }
        }
        Ok(removed)
    }

    fn path_for_digest(&self, digest: &str) -> Result<PathBuf, ArtifactError> {
        let hex_digest = validate_hex_digest(digest)?;
        Ok(self
            .root
            .join(format!("sha256/{}/{hex_digest}", &hex_digest[..2])))
    }
}

pub fn source_image_tag(source_sha256: &str) -> Result<String, ArtifactError> {
    if source_sha256.len() != 64 || !source_sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ArtifactError::InvalidSourceDigest(source_sha256.to_owned()));
    }
    Ok(format!(
        "jitforge-source:{}",
        source_sha256.to_ascii_lowercase()
    ))
}

fn validate_hex_digest(digest: &str) -> Result<&str, ArtifactError> {
    digest
        .strip_prefix("sha256:")
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| ArtifactError::InvalidDigest(digest.to_owned()))
}

fn semantic_digest(bundle: &ArtifactBundle) -> Result<String, ArtifactError> {
    let payload = ArtifactDigestPayload {
        manifest: &bundle.manifest,
        contract: &bundle.contract,
        source: &bundle.source,
        tests: &bundle.tests,
        validation: ValidationDigestPayload {
            tests_total: bundle.validation.tests_total,
            tests_passed: bundle.validation.tests_passed,
            repair_rounds: bundle.validation.repair_rounds,
            agent_turns: bundle.validation.agent_turns,
            generated_test_corrections: bundle.validation.generated_test_corrections,
            probes_run: bundle.validation.probes_run,
        },
    };
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(&payload)?)))
}

fn validate_source(source: &str) -> Result<(), ArtifactError> {
    if source.is_empty() || source.len() > 64 * 1024 {
        return Err(ArtifactError::InvalidSource(
            "source must contain 1-65536 UTF-8 bytes".to_owned(),
        ));
    }
    if source.as_bytes().contains(&0) {
        return Err(ArtifactError::InvalidSource(
            "source must not contain NUL bytes".to_owned(),
        ));
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ArtifactError {
    #[error("invalid artifact digest {0:?}")]
    InvalidDigest(String),

    #[error("invalid source digest {0:?}")]
    InvalidSourceDigest(String),

    #[error("unsupported artifact format version {0}")]
    UnsupportedFormat(u32),

    #[error("invalid generated source: {0}")]
    InvalidSource(String),

    #[error("artifact digest mismatch: expected {expected}, got {actual}")]
    DigestMismatch { expected: String, actual: String },

    #[error(transparent)]
    Io(#[from] io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example_summary() -> ValidationSummary {
        ValidationSummary {
            tests_total: 1,
            tests_passed: 1,
            repair_rounds: 0,
            agent_turns: 1,
            generated_test_corrections: 0,
            probes_run: 0,
            validated_at: "2026-01-01T00:00:00Z".to_owned(),
        }
    }

    #[test]
    fn stores_and_verifies_content_addressed_artifacts() {
        let temporary = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(temporary.path());
        let first = store
            .put(
                IoFormat::Text,
                IoFormat::Text,
                ToolContract {
                    summary: "echo".to_owned(),
                    assumptions: vec![],
                    invariants: vec![],
                },
                "import sys\nsys.stdout.buffer.write(sys.stdin.buffer.read())\n".to_owned(),
                vec![],
                example_summary(),
            )
            .unwrap();
        let second = store.load(&first.digest).unwrap();
        assert_eq!(first.digest, second.digest);
        assert_eq!(first.bundle.source, second.bundle.source);
        assert!(first.directory.join("Dockerfile").is_file());
    }

    #[test]
    fn image_tags_are_derived_from_verified_digests() {
        let digest = "a".repeat(64);
        assert_eq!(
            source_image_tag(&digest).unwrap(),
            format!("jitforge-source:{}", "a".repeat(64))
        );
        assert!(source_image_tag("latest").is_err());
    }

    #[test]
    fn validation_time_is_not_part_of_v2_digest() {
        let first_root = tempfile::tempdir().unwrap();
        let second_root = tempfile::tempdir().unwrap();
        let put = |root: &Path, validated_at: &str| {
            ArtifactStore::new(root)
                .put(
                    IoFormat::Text,
                    IoFormat::Text,
                    ToolContract {
                        summary: "echo".to_owned(),
                        assumptions: vec![],
                        invariants: vec![],
                    },
                    "print('ok')\n".to_owned(),
                    vec![],
                    ValidationSummary {
                        validated_at: validated_at.to_owned(),
                        ..example_summary()
                    },
                )
                .unwrap()
        };
        assert_eq!(
            put(first_root.path(), "2026-01-01T00:00:00Z").digest,
            put(second_root.path(), "2026-07-15T00:00:00Z").digest
        );
    }

    #[test]
    fn loads_existing_v1_bundle_by_raw_bundle_digest() {
        let temporary = tempfile::tempdir().unwrap();
        let bundle = ArtifactBundle {
            manifest: ArtifactManifest {
                format_version: 1,
                runtime: "python-stdlib-v1".to_owned(),
                input_format: IoFormat::Text,
                output_format: IoFormat::Text,
                source_sha256: hex::encode(Sha256::digest(b"print('v1')\n")),
            },
            contract: ToolContract {
                summary: "legacy".to_owned(),
                assumptions: vec![],
                invariants: vec![],
            },
            source: "print('v1')\n".to_owned(),
            tests: vec![],
            validation: example_summary(),
        };
        let encoded = serde_json::to_vec(&bundle).unwrap();
        let hex_digest = hex::encode(Sha256::digest(&encoded));
        let directory = temporary
            .path()
            .join(format!("sha256/{}/{hex_digest}", &hex_digest[..2]));
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("bundle.json"), encoded).unwrap();

        let loaded = ArtifactStore::new(temporary.path())
            .load(&format!("sha256:{hex_digest}"))
            .unwrap();
        assert_eq!(loaded.bundle.manifest.format_version, 1);
        assert_eq!(loaded.bundle.source, "print('v1')\n");
    }
}
