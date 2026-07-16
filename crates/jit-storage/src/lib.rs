use std::{collections::HashSet, str::FromStr, time::Duration};

use chrono::{DateTime, Utc};
use jit_domain::{ToolName, ToolVersionStatus};
use jit_protocol::{
    HttpCapability, HttpCapabilityApproval, HttpCapabilityApprovalList, IoFormat, JobAnswerRequest,
    JobError, JobInputAnswer, JobInputChoice, JobInputKind, JobListResponse, JobResponse, JobStage,
    JobStatus, PendingJobInput, RegistrationRequest, RegistrationResponse, RevokeResponse,
    ToolExample, ToolListItem, ToolListResponse, ToolSummaryResponse, ToolVersionListItem,
    ToolVersionListResponse, ToolVersionSummary,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgPoolOptions};
use thiserror::Error;
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

const AGENT_TRACE_LIMIT_BYTES: i64 = 512 * 1024;
const AGENT_EVENT_PAYLOAD_LIMIT_BYTES: usize = 16 * 1024;
const AGENT_TERMINAL_EVENT_RESERVE_BYTES: i64 = 512;

#[derive(Clone)]
pub struct Registry {
    pool: PgPool,
}

impl Registry {
    pub async fn connect(database_url: &str, max_connections: u32) -> Result<Self, StorageError> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(Duration::from_secs(5))
            .connect(database_url)
            .await?;
        Ok(Self { pool })
    }

    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn migrate(&self) -> Result<(), StorageError> {
        MIGRATOR.run(&self.pool).await?;
        Ok(())
    }

    pub async fn database_ready(&self) -> bool {
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(&self.pool)
            .await
            .is_ok()
    }

    pub async fn has_recent_worker(&self) -> Result<bool, StorageError> {
        let present = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM worker_heartbeats WHERE last_seen_at > now() - interval '30 seconds')",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(present)
    }

    pub async fn register(
        &self,
        name: &ToolName,
        request: &RegistrationRequest,
        idempotency_key: &str,
    ) -> Result<RegistrationResponse, StorageError> {
        let fingerprint = registration_fingerprint(name, request)?;
        let mut transaction = self.pool.begin().await?;

        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(idempotency_key)
            .execute(&mut *transaction)
            .await?;

        if let Some(row) = sqlx::query(
            "SELECT j.request_fingerprint, j.id, t.name, j.revision, v.status
             FROM synthesis_jobs j
             JOIN tools t ON t.id = j.tool_id
             JOIN tool_versions v ON v.tool_id = j.tool_id AND v.revision = j.revision
             WHERE j.idempotency_key = $1",
        )
        .bind(idempotency_key)
        .fetch_optional(&mut *transaction)
        .await?
        {
            let stored_fingerprint: String = row.try_get("request_fingerprint")?;
            if stored_fingerprint != fingerprint {
                return Err(StorageError::IdempotencyConflict);
            }
            let response = RegistrationResponse {
                tool: row.try_get("name")?,
                revision: to_u64(row.try_get::<i64, _>("revision")?, "revision")?,
                status: parse_version_status(row.try_get("status")?)?,
                job_id: row.try_get::<Uuid, _>("id")?.to_string(),
            };
            transaction.commit().await?;
            return Ok(response);
        }

        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 1))")
            .bind(name.as_str())
            .execute(&mut *transaction)
            .await?;

        let tool_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO tools (id, name) VALUES ($1, $2)
             ON CONFLICT (name) DO NOTHING",
        )
        .bind(tool_id)
        .bind(name.as_str())
        .execute(&mut *transaction)
        .await?;

        let row = sqlx::query("SELECT id, latest_revision FROM tools WHERE name = $1 FOR UPDATE")
            .bind(name.as_str())
            .fetch_one(&mut *transaction)
            .await?;
        let tool_id: Uuid = row.try_get("id")?;
        let latest_revision: i64 = row.try_get("latest_revision")?;
        let revision = latest_revision
            .checked_add(1)
            .ok_or_else(|| StorageError::Invariant("revision overflow".to_owned()))?;

        sqlx::query("UPDATE tools SET latest_revision = $2, updated_at = now() WHERE id = $1")
            .bind(tool_id)
            .bind(revision)
            .execute(&mut *transaction)
            .await?;

        sqlx::query(
            "INSERT INTO tool_versions
             (tool_id, revision, requested_intent, description, input_format, output_format, examples, status)
             VALUES ($1, $2, $3, $3, $4, $5, $6, 'draft')",
        )
        .bind(tool_id)
        .bind(revision)
        .bind(&request.intent)
        .bind(request.input_format.as_str())
        .bind(request.output_format.as_str())
        .bind(serde_json::to_value(&request.examples)?)
        .execute(&mut *transaction)
        .await?;

        let job_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO synthesis_jobs
             (id, tool_id, revision, idempotency_key, request_fingerprint, input_samples, status, stage)
             VALUES ($1, $2, $3, $4, $5, $6, 'queued', 'queued')",
        )
        .bind(job_id)
        .bind(tool_id)
        .bind(revision)
        .bind(idempotency_key)
        .bind(fingerprint)
        .bind(serde_json::to_value(&request.input_samples)?)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(RegistrationResponse {
            tool: name.to_string(),
            revision: to_u64(revision, "revision")?,
            status: ToolVersionStatus::Draft,
            job_id: job_id.to_string(),
        })
    }

    pub async fn get_job(&self, job_id: Uuid) -> Result<JobResponse, StorageError> {
        let row = sqlx::query(
            "SELECT j.id, t.name, j.revision, j.status AS job_status, j.stage,
                    v.status AS version_status, j.error_code, j.error_message, j.details,
                    j.created_at, j.updated_at,
                    qi.id AS input_id, qi.kind AS input_kind, qi.prompt AS input_prompt,
                    qi.choices AS input_choices, qi.context AS input_context,
                    qi.created_at AS input_created_at
             FROM synthesis_jobs j
             JOIN tools t ON t.id = j.tool_id
             JOIN tool_versions v ON v.tool_id = j.tool_id AND v.revision = j.revision
             LEFT JOIN synthesis_job_inputs qi ON qi.job_id = j.id AND qi.status = 'pending'
             WHERE j.id = $1",
        )
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StorageError::JobNotFound)?;
        map_job(row)
    }

    pub async fn list_jobs(
        &self,
        status: Option<JobStatus>,
        limit: u32,
        offset: u64,
    ) -> Result<JobListResponse, StorageError> {
        let row_limit = i64::from(limit)
            .checked_add(1)
            .ok_or_else(|| StorageError::Invariant("job list limit overflow".to_owned()))?;
        let offset = i64::try_from(offset).map_err(|_| {
            StorageError::Invariant("job list offset exceeds PostgreSQL BIGINT".to_owned())
        })?;
        let mut rows = sqlx::query(
            "SELECT j.id, t.name, j.revision, j.status AS job_status, j.stage,
                    v.status AS version_status, j.error_code, j.error_message, j.details,
                    j.created_at, j.updated_at,
                    qi.id AS input_id, qi.kind AS input_kind, qi.prompt AS input_prompt,
                    qi.choices AS input_choices, qi.context AS input_context,
                    qi.created_at AS input_created_at
             FROM synthesis_jobs j
             JOIN tools t ON t.id = j.tool_id
             JOIN tool_versions v ON v.tool_id = j.tool_id AND v.revision = j.revision
             LEFT JOIN synthesis_job_inputs qi ON qi.job_id = j.id AND qi.status = 'pending'
             WHERE ($1::text IS NULL OR j.status = $1)
             ORDER BY j.created_at DESC, j.id DESC
             LIMIT $2 OFFSET $3",
        )
        .bind(status.map(JobStatus::as_str))
        .bind(row_limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        let has_more = rows.len() > limit as usize;
        rows.truncate(limit as usize);
        let jobs = rows
            .into_iter()
            .map(map_job)
            .collect::<Result<Vec<_>, StorageError>>()?;
        Ok(JobListResponse {
            jobs,
            next_offset: next_offset(has_more, offset, limit),
        })
    }

    pub async fn list_tools(
        &self,
        query: &str,
        include_unready: bool,
        status: Option<ToolVersionStatus>,
        limit: u32,
        offset: u64,
    ) -> Result<ToolListResponse, StorageError> {
        let query = query.trim();
        let row_limit = i64::from(limit)
            .checked_add(1)
            .ok_or_else(|| StorageError::Invariant("tool list limit overflow".to_owned()))?;
        let offset = i64::try_from(offset).map_err(|_| {
            StorageError::Invariant("tool list offset exceeds PostgreSQL BIGINT".to_owned())
        })?;
        let mut rows = sqlx::query(
            "SELECT t.name, t.latest_revision, t.stable_revision,
                    v.revision AS selected_revision, v.description, v.status,
                    v.input_format, v.output_format
             FROM tools t
             JOIN tool_versions v
               ON v.tool_id = t.id
              AND v.revision = COALESCE(t.stable_revision, t.latest_revision)
             WHERE ($1 OR t.stable_revision IS NOT NULL)
               AND ($2::text IS NULL OR v.status = $2)
               AND ($3 = ''
                    OR strpos(lower(t.name), lower($3)) > 0
                    OR strpos(lower(v.description), lower($3)) > 0)
             ORDER BY CASE
                        WHEN lower(t.name) = lower($3) THEN 0
                        WHEN left(lower(t.name), char_length($3)) = lower($3) THEN 1
                        ELSE 2
                      END,
                      t.name
             LIMIT $4 OFFSET $5",
        )
        .bind(include_unready)
        .bind(status.map(ToolVersionStatus::as_str))
        .bind(query)
        .bind(row_limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        let has_more = rows.len() > limit as usize;
        rows.truncate(limit as usize);
        let tools = rows
            .into_iter()
            .map(|row| {
                let stable_revision = row
                    .try_get::<Option<i64>, _>("stable_revision")?
                    .map(|value| to_u64(value, "stable_revision"))
                    .transpose()?;
                Ok(ToolListItem {
                    tool: row.try_get("name")?,
                    stable_revision,
                    latest_revision: to_u64(row.try_get("latest_revision")?, "latest_revision")?,
                    selected_revision: to_u64(
                        row.try_get("selected_revision")?,
                        "selected_revision",
                    )?,
                    description: row.try_get("description")?,
                    status: parse_version_status(row.try_get("status")?)?,
                    input_format: parse_io_format(row.try_get("input_format")?)?,
                    output_format: parse_io_format(row.try_get("output_format")?)?,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let next_offset = has_more
            .then(|| {
                u64::try_from(offset)
                    .ok()
                    .and_then(|offset| offset.checked_add(u64::from(limit)))
            })
            .flatten();
        Ok(ToolListResponse { tools, next_offset })
    }

    pub async fn inspect_tool(
        &self,
        name: &ToolName,
        requested_revision: Option<u64>,
    ) -> Result<ToolSummaryResponse, StorageError> {
        let tool =
            sqlx::query("SELECT id, latest_revision, stable_revision FROM tools WHERE name = $1")
                .bind(name.as_str())
                .fetch_optional(&self.pool)
                .await?
                .ok_or(StorageError::ToolNotFound)?;

        let tool_id: Uuid = tool.try_get("id")?;
        let latest_revision = to_u64(tool.try_get("latest_revision")?, "latest_revision")?;
        let stable_revision = tool
            .try_get::<Option<i64>, _>("stable_revision")?
            .map(|value| to_u64(value, "stable_revision"))
            .transpose()?;
        let selected_revision = requested_revision
            .or(stable_revision)
            .unwrap_or(latest_revision);
        let selected_revision_i64 = i64::try_from(selected_revision).map_err(|_| {
            StorageError::Invariant("revision exceeds PostgreSQL BIGINT".to_owned())
        })?;

        let version = sqlx::query(
            "SELECT revision, requested_intent, description, status, input_format, output_format, assumptions,
                    contract, artifact_digest, validation_summary, error_code, error_message
             FROM tool_versions WHERE tool_id = $1 AND revision = $2",
        )
        .bind(tool_id)
        .bind(selected_revision_i64)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StorageError::VersionNotFound)?;

        let assumptions: Value = version.try_get("assumptions")?;
        let error_code: Option<String> = version.try_get("error_code")?;
        let error_message: Option<String> = version.try_get("error_message")?;
        let error = match (error_code, error_message) {
            (Some(code), Some(message)) => Some(JobError {
                code,
                message,
                details: None,
            }),
            _ => None,
        };

        Ok(ToolSummaryResponse {
            tool: name.to_string(),
            stable_revision,
            latest_revision,
            selected: ToolVersionSummary {
                revision: to_u64(version.try_get("revision")?, "revision")?,
                requested_intent: version.try_get("requested_intent")?,
                description: version.try_get("description")?,
                status: parse_version_status(version.try_get("status")?)?,
                input_format: parse_io_format(version.try_get("input_format")?)?,
                output_format: parse_io_format(version.try_get("output_format")?)?,
                assumptions: serde_json::from_value(assumptions)?,
                contract: version.try_get("contract")?,
                artifact_digest: version.try_get("artifact_digest")?,
                validation_summary: version.try_get("validation_summary")?,
                error,
            },
        })
    }

    pub async fn list_tool_versions(
        &self,
        name: &ToolName,
        limit: u32,
        offset: u64,
    ) -> Result<ToolVersionListResponse, StorageError> {
        let tool =
            sqlx::query("SELECT id, latest_revision, stable_revision FROM tools WHERE name = $1")
                .bind(name.as_str())
                .fetch_optional(&self.pool)
                .await?
                .ok_or(StorageError::ToolNotFound)?;
        let tool_id: Uuid = tool.try_get("id")?;
        let latest_revision = to_u64(tool.try_get("latest_revision")?, "latest_revision")?;
        let stable_revision = tool
            .try_get::<Option<i64>, _>("stable_revision")?
            .map(|value| to_u64(value, "stable_revision"))
            .transpose()?;
        let row_limit = i64::from(limit)
            .checked_add(1)
            .ok_or_else(|| StorageError::Invariant("version list limit overflow".to_owned()))?;
        let offset = i64::try_from(offset).map_err(|_| {
            StorageError::Invariant("version list offset exceeds PostgreSQL BIGINT".to_owned())
        })?;
        let mut rows = sqlx::query(
            "SELECT revision, description, status, input_format, output_format,
                    artifact_digest, error_code, error_message, created_at, updated_at
             FROM tool_versions WHERE tool_id = $1
             ORDER BY revision DESC LIMIT $2 OFFSET $3",
        )
        .bind(tool_id)
        .bind(row_limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        let has_more = rows.len() > limit as usize;
        rows.truncate(limit as usize);
        let versions = rows
            .into_iter()
            .map(|row| {
                let error_code: Option<String> = row.try_get("error_code")?;
                let error_message: Option<String> = row.try_get("error_message")?;
                let error = match (error_code, error_message) {
                    (Some(code), Some(message)) => Some(JobError {
                        code,
                        message,
                        details: None,
                    }),
                    _ => None,
                };
                let created_at: DateTime<Utc> = row.try_get("created_at")?;
                let updated_at: DateTime<Utc> = row.try_get("updated_at")?;
                Ok(ToolVersionListItem {
                    revision: to_u64(row.try_get("revision")?, "revision")?,
                    description: row.try_get("description")?,
                    status: parse_version_status(row.try_get("status")?)?,
                    input_format: parse_io_format(row.try_get("input_format")?)?,
                    output_format: parse_io_format(row.try_get("output_format")?)?,
                    artifact_digest: row.try_get("artifact_digest")?,
                    error,
                    created_at: created_at.to_rfc3339(),
                    updated_at: updated_at.to_rfc3339(),
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        Ok(ToolVersionListResponse {
            tool: name.to_string(),
            stable_revision,
            latest_revision,
            versions,
            next_offset: next_offset(has_more, offset, limit),
        })
    }

    pub async fn artifact_digest_for_version(
        &self,
        name: &ToolName,
        revision: u64,
    ) -> Result<String, StorageError> {
        let revision = i64::try_from(revision).map_err(|_| StorageError::VersionNotFound)?;
        let row = sqlx::query(
            "SELECT v.artifact_digest
             FROM tools t
             JOIN tool_versions v ON v.tool_id = t.id
             WHERE t.name = $1 AND v.revision = $2",
        )
        .bind(name.as_str())
        .bind(revision)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StorageError::VersionNotFound)?;
        row.try_get::<Option<String>, _>("artifact_digest")?
            .ok_or(StorageError::ArtifactNotFound)
    }

    pub async fn revoke_tool_version(
        &self,
        name: &ToolName,
        revision: u64,
        reason: &str,
    ) -> Result<RevokeResponse, StorageError> {
        let revision = i64::try_from(revision).map_err(|_| StorageError::VersionNotFound)?;
        let mut transaction = self.pool.begin().await?;
        let tool = sqlx::query("SELECT id, stable_revision FROM tools WHERE name = $1 FOR UPDATE")
            .bind(name.as_str())
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(StorageError::ToolNotFound)?;
        let tool_id: Uuid = tool.try_get("id")?;
        let stable_revision: Option<i64> = tool.try_get("stable_revision")?;
        let version = sqlx::query(
            "SELECT status FROM tool_versions
             WHERE tool_id = $1 AND revision = $2 FOR UPDATE",
        )
        .bind(tool_id)
        .bind(revision)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(StorageError::VersionNotFound)?;
        let status: String = version.try_get("status")?;
        if !matches!(status.as_str(), "ready" | "deprecated") {
            return Err(StorageError::VersionNotRevocable(status));
        }

        sqlx::query(
            "UPDATE tool_versions
             SET status = 'revoked', error_code = 'revoked', error_message = $3,
                 updated_at = now()
             WHERE tool_id = $1 AND revision = $2",
        )
        .bind(tool_id)
        .bind(revision)
        .bind(reason)
        .execute(&mut *transaction)
        .await?;

        let next_stable = if stable_revision == Some(revision) {
            let fallback = sqlx::query_scalar::<_, Option<i64>>(
                "SELECT max(revision) FROM tool_versions
                 WHERE tool_id = $1 AND status = 'ready' AND revision <> $2",
            )
            .bind(tool_id)
            .bind(revision)
            .fetch_one(&mut *transaction)
            .await?;
            sqlx::query("UPDATE tools SET stable_revision = $2, updated_at = now() WHERE id = $1")
                .bind(tool_id)
                .bind(fallback)
                .execute(&mut *transaction)
                .await?;
            fallback
        } else {
            stable_revision
        };
        transaction.commit().await?;
        Ok(RevokeResponse {
            tool: name.to_string(),
            revision: to_u64(revision, "revision")?,
            status: ToolVersionStatus::Revoked,
            stable_revision: next_stable
                .map(|value| to_u64(value, "stable_revision"))
                .transpose()?,
        })
    }

    pub async fn record_worker_heartbeat(
        &self,
        worker_id: &str,
        version: &str,
    ) -> Result<(), StorageError> {
        sqlx::query(
            "INSERT INTO worker_heartbeats (worker_id, version, last_seen_at)
             VALUES ($1, $2, now())
             ON CONFLICT (worker_id) DO UPDATE
             SET version = excluded.version, last_seen_at = excluded.last_seen_at",
        )
        .bind(worker_id)
        .bind(version)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn is_http_capability_approved(
        &self,
        capability_hash: &str,
    ) -> Result<bool, StorageError> {
        let approved = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(
                SELECT 1 FROM http_capability_approvals
                WHERE capability_hash = $1 AND status = 'active'
            )",
        )
        .bind(capability_hash)
        .fetch_one(&self.pool)
        .await?;
        Ok(approved)
    }

    pub async fn approve_http_capability(
        &self,
        capability_hash: &str,
        capability: &HttpCapability,
        job_id: Uuid,
    ) -> Result<(), StorageError> {
        sqlx::query(
            "INSERT INTO http_capability_approvals
             (capability_hash, capability, status, approved_by_job)
             VALUES ($1, $2, 'active', $3)
             ON CONFLICT (capability_hash) DO UPDATE
             SET capability = excluded.capability,
                 status = 'active',
                 approved_by_job = excluded.approved_by_job,
                 approved_at = now(),
                 revoked_at = NULL",
        )
        .bind(capability_hash)
        .bind(serde_json::to_value(capability)?)
        .bind(job_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn all_http_capabilities_approved(
        &self,
        capability_hashes: &[String],
    ) -> Result<bool, StorageError> {
        if capability_hashes.is_empty() {
            return Ok(true);
        }
        let active: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM http_capability_approvals
             WHERE capability_hash = ANY($1) AND status = 'active'",
        )
        .bind(capability_hashes)
        .fetch_one(&self.pool)
        .await?;
        Ok(active == i64::try_from(capability_hashes.len()).unwrap_or(i64::MAX))
    }

    pub async fn list_http_capability_approvals(
        &self,
    ) -> Result<HttpCapabilityApprovalList, StorageError> {
        let rows = sqlx::query(
            "SELECT capability_hash, capability, status, approved_at, revoked_at, revoked_reason
             FROM http_capability_approvals
             ORDER BY approved_at DESC, capability_hash",
        )
        .fetch_all(&self.pool)
        .await?;
        let approvals = rows
            .into_iter()
            .map(|row| {
                let capability: Value = row.try_get("capability")?;
                let approved_at: DateTime<Utc> = row.try_get("approved_at")?;
                let revoked_at: Option<DateTime<Utc>> = row.try_get("revoked_at")?;
                Ok(HttpCapabilityApproval {
                    capability_hash: row.try_get("capability_hash")?,
                    capability: serde_json::from_value(capability)?,
                    status: row.try_get("status")?,
                    approved_at: approved_at.to_rfc3339(),
                    revoked_at: revoked_at.map(|value| value.to_rfc3339()),
                    revoked_reason: row.try_get("revoked_reason")?,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        Ok(HttpCapabilityApprovalList { approvals })
    }

    pub async fn revoke_http_capability(
        &self,
        capability_hash: &str,
        reason: &str,
    ) -> Result<(), StorageError> {
        if reason.trim().is_empty() || reason.len() > 4096 {
            return Err(StorageError::InvalidRevocationReason);
        }
        let result = sqlx::query(
            "UPDATE http_capability_approvals
             SET status = 'revoked', revoked_at = now(), revoked_reason = $2
             WHERE capability_hash = $1 AND status = 'active'",
        )
        .bind(capability_hash)
        .bind(reason.trim())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StorageError::HttpCapabilityNotFound);
        }
        Ok(())
    }

    pub async fn claim_synthesis_job(
        &self,
        worker_id: &str,
        lease_seconds: i64,
    ) -> Result<Option<ClaimedSynthesisJob>, StorageError> {
        let mut transaction = self.pool.begin().await?;
        let candidate = sqlx::query(
            "SELECT id FROM synthesis_jobs
             WHERE (status = 'queued' AND available_at <= now())
                OR (status = 'running' AND lease_until < now())
             ORDER BY created_at
             FOR UPDATE SKIP LOCKED
             LIMIT 1",
        )
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(candidate) = candidate else {
            transaction.commit().await?;
            return Ok(None);
        };
        let job_id: Uuid = candidate.try_get("id")?;

        sqlx::query(
            "UPDATE synthesis_jobs
             SET status = 'running',
                 stage = CASE WHEN status = 'queued' AND stage = 'queued' THEN 'contract' ELSE stage END,
                 attempts = attempts + CASE
                     WHEN status = 'queued' AND stage <> 'queued' THEN 0
                     ELSE 1
                 END,
                 worker_id = $2,
                 lease_until = now() + ($3::bigint * interval '1 second'),
                 updated_at = now()
             WHERE id = $1",
        )
        .bind(job_id)
        .bind(worker_id)
        .bind(lease_seconds)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE tool_versions v SET status = 'synthesizing', updated_at = now()
             FROM synthesis_jobs j
             WHERE j.id = $1 AND v.tool_id = j.tool_id AND v.revision = j.revision
               AND v.status IN ('draft', 'contract_ready', 'synthesizing')",
        )
        .bind(job_id)
        .execute(&mut *transaction)
        .await?;

        let row = sqlx::query(
            "SELECT j.id, j.tool_id, t.name, j.revision, j.attempts,
                    v.requested_intent, v.input_format, v.output_format, v.examples, j.input_samples,
                    ar.engine AS agent_engine, ar.engine_version AS agent_engine_version,
                    ar.checkpoint AS agent_checkpoint
             FROM synthesis_jobs j
             JOIN tools t ON t.id = j.tool_id
             JOIN tool_versions v ON v.tool_id = j.tool_id AND v.revision = j.revision
             LEFT JOIN synthesis_agent_runs ar ON ar.job_id = j.id
             WHERE j.id = $1",
        )
        .bind(job_id)
        .fetch_one(&mut *transaction)
        .await?;
        let examples: Value = row.try_get("examples")?;
        let input_samples: Value = row.try_get("input_samples")?;
        let agent_engine: Option<String> = row.try_get("agent_engine")?;
        let agent_engine_version: Option<String> = row.try_get("agent_engine_version")?;
        let agent_checkpoint: Option<Value> = row.try_get("agent_checkpoint")?;
        let agent_checkpoint = match (agent_engine, agent_engine_version, agent_checkpoint) {
            (Some(engine), Some(engine_version), Some(checkpoint)) => Some(AgentCheckpointRecord {
                engine,
                engine_version,
                checkpoint,
            }),
            (None, None, None) => None,
            _ => {
                return Err(StorageError::Invariant(
                    "partial synthesis agent checkpoint".to_owned(),
                ));
            }
        };
        let claimed = ClaimedSynthesisJob {
            job_id,
            tool_id: row.try_get("tool_id")?,
            tool: row.try_get("name")?,
            revision: to_u64(row.try_get("revision")?, "revision")?,
            intent: row.try_get("requested_intent")?,
            input_format: parse_io_format(row.try_get("input_format")?)?,
            output_format: parse_io_format(row.try_get("output_format")?)?,
            examples: serde_json::from_value(examples)?,
            input_samples: serde_json::from_value(input_samples)?,
            attempts: row.try_get::<i32, _>("attempts")? as u32,
            agent_checkpoint,
        };
        transaction.commit().await?;
        Ok(Some(claimed))
    }

    pub async fn renew_job_lease(
        &self,
        job_id: Uuid,
        worker_id: &str,
        lease_seconds: i64,
    ) -> Result<(), StorageError> {
        let result = sqlx::query(
            "UPDATE synthesis_jobs
             SET lease_until = now() + ($3::bigint * interval '1 second'), updated_at = now()
             WHERE id = $1 AND worker_id = $2 AND status = 'running'",
        )
        .bind(job_id)
        .bind(worker_id)
        .bind(lease_seconds)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StorageError::JobLeaseLost);
        }
        Ok(())
    }

    pub async fn cancel_job(&self, job_id: Uuid, reason: &str) -> Result<(), StorageError> {
        let reason = reason.trim();
        if reason.is_empty() || reason.len() > 4096 {
            return Err(StorageError::InvalidCancellationReason);
        }
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query("SELECT status FROM synthesis_jobs WHERE id = $1 FOR UPDATE")
            .bind(job_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(StorageError::JobNotFound)?;
        let status: String = row.try_get("status")?;
        if matches!(status.as_str(), "ready" | "rejected") {
            return Err(StorageError::JobNotCancellable(status));
        }
        let details = serde_json::json!({"reason": reason});
        sqlx::query(
            "UPDATE synthesis_jobs
             SET status = 'rejected', stage = 'complete', error_code = 'user_cancelled',
                 error_message = 'synthesis job cancelled by user', details = $2,
                 worker_id = NULL, lease_until = NULL, updated_at = now()
             WHERE id = $1",
        )
        .bind(job_id)
        .bind(&details)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE tool_versions v
             SET status = 'rejected', error_code = 'user_cancelled',
                 error_message = 'synthesis job cancelled by user', updated_at = now()
             FROM synthesis_jobs j
             WHERE j.id = $1 AND v.tool_id = j.tool_id AND v.revision = j.revision",
        )
        .bind(job_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE synthesis_job_inputs
             SET status = 'answered', answer = $2, answered_at = now()
             WHERE job_id = $1 AND status = 'pending'",
        )
        .bind(job_id)
        .bind(serde_json::json!({"type": "reject", "reason": reason}))
        .execute(&mut *transaction)
        .await?;
        append_agent_event_in_transaction(
            &mut transaction,
            job_id,
            "job_cancelled",
            &details,
            AGENT_TRACE_LIMIT_BYTES,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn save_agent_checkpoint(
        &self,
        job_id: Uuid,
        worker_id: &str,
        engine: &str,
        engine_version: &str,
        checkpoint: &Value,
    ) -> Result<(), StorageError> {
        let mut transaction = self.pool.begin().await?;
        ensure_job_owner(&mut transaction, job_id, worker_id).await?;
        sqlx::query(
            "INSERT INTO synthesis_agent_runs
             (job_id, engine, engine_version, checkpoint)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (job_id) DO UPDATE
             SET engine = excluded.engine,
                 engine_version = excluded.engine_version,
                 checkpoint = excluded.checkpoint,
                 updated_at = now()",
        )
        .bind(job_id)
        .bind(engine)
        .bind(engine_version)
        .bind(checkpoint)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn suspend_job_for_input(
        &self,
        job_id: Uuid,
        worker_id: &str,
        input: &NewJobInput,
    ) -> Result<Uuid, StorageError> {
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT stage FROM synthesis_jobs
             WHERE id = $1 AND worker_id = $2 AND status = 'running'
             FOR UPDATE",
        )
        .bind(job_id)
        .bind(worker_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(StorageError::JobLeaseLost)?;
        let resume_stage: String = row.try_get("stage")?;
        if matches!(
            resume_stage.as_str(),
            "queued" | "awaiting_input" | "complete"
        ) {
            return Err(StorageError::Invariant(format!(
                "cannot suspend a job from stage {resume_stage:?}"
            )));
        }

        if let Some(existing) = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM synthesis_job_inputs
             WHERE job_id = $1 AND agent_call_id = $2",
        )
        .bind(job_id)
        .bind(&input.agent_call_id)
        .fetch_optional(&mut *transaction)
        .await?
        {
            transaction.commit().await?;
            return Ok(existing);
        }

        sqlx::query(
            "INSERT INTO synthesis_job_inputs
             (id, job_id, agent_call_id, kind, prompt, choices, context, resume_stage)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(input.id)
        .bind(job_id)
        .bind(&input.agent_call_id)
        .bind(input.kind.as_str())
        .bind(&input.prompt)
        .bind(serde_json::to_value(&input.choices)?)
        .bind(&input.context)
        .bind(&resume_stage)
        .execute(&mut *transaction)
        .await?;
        append_agent_event_in_transaction(
            &mut transaction,
            job_id,
            "input_requested",
            &serde_json::json!({
                "input_id": input.id,
                "kind": input.kind,
                "prompt": input.prompt,
                "context": input.context
            }),
            AGENT_TRACE_LIMIT_BYTES - AGENT_TERMINAL_EVENT_RESERVE_BYTES,
        )
        .await?;
        sqlx::query(
            "UPDATE synthesis_jobs
             SET status = 'awaiting_input', stage = 'awaiting_input',
                 worker_id = NULL, lease_until = NULL, updated_at = now()
             WHERE id = $1",
        )
        .bind(job_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(input.id)
    }

    pub async fn answered_job_input(
        &self,
        job_id: Uuid,
        agent_call_id: &str,
    ) -> Result<Option<JobInputAnswer>, StorageError> {
        let answer = sqlx::query_scalar::<_, Value>(
            "SELECT answer FROM synthesis_job_inputs
             WHERE job_id = $1 AND agent_call_id = $2 AND status = 'answered'",
        )
        .bind(job_id)
        .bind(agent_call_id)
        .fetch_optional(&self.pool)
        .await?;
        answer
            .map(serde_json::from_value)
            .transpose()
            .map_err(Into::into)
    }

    pub async fn answer_job_input(
        &self,
        job_id: Uuid,
        request: &JobAnswerRequest,
    ) -> Result<JobResponse, StorageError> {
        let input_id =
            Uuid::parse_str(&request.input_id).map_err(|_| StorageError::JobInputNotFound)?;
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT i.kind, i.resume_stage
             FROM synthesis_job_inputs i
             JOIN synthesis_jobs j ON j.id = i.job_id
             WHERE i.id = $1 AND i.job_id = $2 AND i.status = 'pending'
               AND j.status = 'awaiting_input'
             FOR UPDATE OF i, j",
        )
        .bind(input_id)
        .bind(job_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(StorageError::JobInputNotFound)?;
        let kind = parse_job_input_kind(row.try_get("kind")?)?;
        validate_job_input_answer(kind, &request.answer)?;
        let resume_stage: String = row.try_get("resume_stage")?;
        let answer = serde_json::to_value(&request.answer)?;

        sqlx::query(
            "UPDATE synthesis_job_inputs
             SET status = 'answered', answer = $2, answered_at = now()
             WHERE id = $1",
        )
        .bind(input_id)
        .bind(&answer)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE synthesis_jobs
             SET status = 'queued', stage = $2, available_at = now(),
                 worker_id = NULL, lease_until = NULL, updated_at = now()
             WHERE id = $1",
        )
        .bind(job_id)
        .bind(resume_stage)
        .execute(&mut *transaction)
        .await?;
        append_agent_event_in_transaction(
            &mut transaction,
            job_id,
            "input_answered",
            &serde_json::json!({"input_id": input_id, "answer": answer}),
            AGENT_TRACE_LIMIT_BYTES - AGENT_TERMINAL_EVENT_RESERVE_BYTES,
        )
        .await?;
        transaction.commit().await?;
        self.get_job(job_id).await
    }

    pub async fn append_agent_event(
        &self,
        job_id: Uuid,
        worker_id: &str,
        kind: &str,
        payload: &Value,
    ) -> Result<(), StorageError> {
        let mut transaction = self.pool.begin().await?;
        ensure_job_owner(&mut transaction, job_id, worker_id).await?;
        append_agent_event_in_transaction(
            &mut transaction,
            job_id,
            kind,
            payload,
            AGENT_TRACE_LIMIT_BYTES - AGENT_TERMINAL_EVENT_RESERVE_BYTES,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn referenced_artifacts(
        &self,
    ) -> Result<(HashSet<String>, HashSet<String>), StorageError> {
        self.referenced_artifacts_inner(None).await
    }

    pub async fn referenced_artifacts_excluding_job(
        &self,
        job_id: Uuid,
    ) -> Result<(HashSet<String>, HashSet<String>), StorageError> {
        self.referenced_artifacts_inner(Some(job_id)).await
    }

    async fn referenced_artifacts_inner(
        &self,
        excluded_job_id: Option<Uuid>,
    ) -> Result<(HashSet<String>, HashSet<String>), StorageError> {
        let rows = sqlx::query("SELECT digest, manifest FROM artifacts")
            .fetch_all(&self.pool)
            .await?;
        let mut digests = HashSet::new();
        let mut sources = HashSet::new();
        for row in rows {
            digests.insert(row.try_get::<String, _>("digest")?);
            let manifest: Value = row.try_get("manifest")?;
            if let Some(source) = manifest.get("source_sha256").and_then(Value::as_str) {
                sources.insert(source.to_owned());
            }
        }
        let active = sqlx::query(
            "SELECT ar.checkpoint
             FROM synthesis_agent_runs ar
             JOIN synthesis_jobs j ON j.id = ar.job_id
             WHERE j.status IN ('running', 'awaiting_input')
               AND ($1::uuid IS NULL OR j.id <> $1)",
        )
        .bind(excluded_job_id)
        .fetch_all(&self.pool)
        .await?;
        for row in active {
            let checkpoint: Value = row.try_get("checkpoint")?;
            if let Some(values) = checkpoint
                .pointer("/workspace/candidate_digests")
                .and_then(Value::as_array)
            {
                digests.extend(values.iter().filter_map(Value::as_str).map(str::to_owned));
            }
            if let Some(values) = checkpoint
                .pointer("/workspace/candidate_sources")
                .and_then(Value::as_array)
            {
                sources.extend(values.iter().filter_map(Value::as_str).map(str::to_owned));
            }
        }
        Ok((digests, sources))
    }

    pub async fn compact_agent_traces(&self) -> Result<u64, StorageError> {
        let mut transaction = self.pool.begin().await?;
        let archived = sqlx::query(
            "INSERT INTO synthesis_agent_trace_archives (job_id, event_count, events)
             SELECT e.job_id, count(*)::integer,
                    jsonb_agg(jsonb_build_object(
                        'id', e.id,
                        'kind', e.kind,
                        'payload', e.payload,
                        'created_at', e.created_at
                    ) ORDER BY e.id)
             FROM synthesis_agent_events e
             JOIN synthesis_jobs j ON j.id = e.job_id
             WHERE j.status IN ('ready', 'rejected')
               AND j.updated_at < now() - interval '7 days'
             GROUP BY e.job_id
             ON CONFLICT (job_id) DO NOTHING",
        )
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        sqlx::query(
            "DELETE FROM synthesis_agent_events e
             USING synthesis_agent_trace_archives a
             WHERE e.job_id = a.job_id",
        )
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(archived)
    }

    pub async fn set_job_stage(
        &self,
        job_id: Uuid,
        worker_id: &str,
        stage: JobStage,
    ) -> Result<(), StorageError> {
        let result = sqlx::query(
            "UPDATE synthesis_jobs
             SET stage = $3, lease_until = now() + interval '300 seconds', updated_at = now()
             WHERE id = $1 AND worker_id = $2 AND status = 'running'",
        )
        .bind(job_id)
        .bind(worker_id)
        .bind(stage.as_str())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StorageError::JobLeaseLost);
        }
        let version_status = match stage {
            JobStage::Building => Some("building"),
            JobStage::Validating => Some("validating"),
            JobStage::Contract | JobStage::Synthesizing | JobStage::Repairing => {
                Some("synthesizing")
            }
            JobStage::Queued | JobStage::AwaitingInput | JobStage::Complete => None,
        };
        if let Some(version_status) = version_status {
            sqlx::query(
                "UPDATE tool_versions v SET status = $2, updated_at = now()
                 FROM synthesis_jobs j
                 WHERE j.id = $1 AND v.tool_id = j.tool_id AND v.revision = j.revision",
            )
            .bind(job_id)
            .bind(version_status)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    pub async fn save_contract(
        &self,
        job_id: Uuid,
        worker_id: &str,
        contract: &Value,
        assumptions: &[String],
    ) -> Result<(), StorageError> {
        let result = sqlx::query(
            "UPDATE tool_versions v
             SET contract = $3, description = $3->>'summary', assumptions = $4, updated_at = now()
             FROM synthesis_jobs j
             WHERE j.id = $1 AND j.worker_id = $2 AND j.status = 'running'
               AND v.tool_id = j.tool_id AND v.revision = j.revision",
        )
        .bind(job_id)
        .bind(worker_id)
        .bind(contract)
        .bind(serde_json::to_value(assumptions)?)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StorageError::JobLeaseLost);
        }
        Ok(())
    }

    pub async fn publish_job(
        &self,
        job_id: Uuid,
        worker_id: &str,
        artifact: &PublishedArtifact,
    ) -> Result<(), StorageError> {
        let mut transaction = self.pool.begin().await?;
        let owner = sqlx::query(
            "SELECT tool_id, revision FROM synthesis_jobs
             WHERE id = $1 AND worker_id = $2 AND status = 'running'
             FOR UPDATE",
        )
        .bind(job_id)
        .bind(worker_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(StorageError::JobLeaseLost)?;
        let tool_id: Uuid = owner.try_get("tool_id")?;
        let revision: i64 = owner.try_get("revision")?;

        sqlx::query(
            "INSERT INTO artifacts (digest, relative_path, size_bytes, manifest, validation_summary)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (digest) DO NOTHING",
        )
        .bind(&artifact.digest)
        .bind(&artifact.relative_path)
        .bind(artifact.size_bytes)
        .bind(&artifact.manifest)
        .bind(&artifact.validation_summary)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            "UPDATE tool_versions
             SET status = 'ready', contract = $3, assumptions = $4,
                 artifact_digest = $5, validation_summary = $6,
                 error_code = NULL, error_message = NULL, updated_at = now()
             WHERE tool_id = $1 AND revision = $2",
        )
        .bind(tool_id)
        .bind(revision)
        .bind(&artifact.contract)
        .bind(&artifact.assumptions)
        .bind(&artifact.digest)
        .bind(&artifact.validation_summary)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("UPDATE tools SET stable_revision = $2, updated_at = now() WHERE id = $1")
            .bind(tool_id)
            .bind(revision)
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "UPDATE synthesis_jobs
             SET status = 'ready', stage = 'complete', lease_until = NULL, updated_at = now()
             WHERE id = $1",
        )
        .bind(job_id)
        .execute(&mut *transaction)
        .await?;
        append_agent_event_in_transaction(
            &mut transaction,
            job_id,
            "candidate_validated",
            &serde_json::json!({"digest": artifact.digest}),
            AGENT_TRACE_LIMIT_BYTES,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn reject_job(
        &self,
        job_id: Uuid,
        worker_id: &str,
        code: &str,
        message: &str,
        details: Option<&Value>,
    ) -> Result<(), StorageError> {
        let mut transaction = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE synthesis_jobs
             SET status = 'rejected', stage = 'complete', error_code = $3,
                 error_message = $4, details = $5, lease_until = NULL, updated_at = now()
             WHERE id = $1 AND worker_id = $2 AND status = 'running'",
        )
        .bind(job_id)
        .bind(worker_id)
        .bind(code)
        .bind(message)
        .bind(details)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StorageError::JobLeaseLost);
        }
        sqlx::query(
            "UPDATE tool_versions v
             SET status = 'rejected', error_code = $2, error_message = $3, updated_at = now()
             FROM synthesis_jobs j
             WHERE j.id = $1 AND v.tool_id = j.tool_id AND v.revision = j.revision",
        )
        .bind(job_id)
        .bind(code)
        .bind(message)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn resolve_tool(
        &self,
        name: &ToolName,
        requested_revision: Option<u64>,
    ) -> Result<ResolvedTool, StorageError> {
        let revision = requested_revision
            .map(i64::try_from)
            .transpose()
            .map_err(|_| StorageError::VersionNotFound)?;
        let row = sqlx::query(
            "SELECT t.id AS tool_id, COALESCE($2, t.stable_revision) AS selected_revision,
                    v.status, v.artifact_digest, v.input_format, v.output_format
             FROM tools t
             LEFT JOIN tool_versions v
               ON v.tool_id = t.id AND v.revision = COALESCE($2, t.stable_revision)
             WHERE t.name = $1",
        )
        .bind(name.as_str())
        .bind(revision)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StorageError::ToolNotFound)?;
        let selected_revision: Option<i64> = row.try_get("selected_revision")?;
        let selected_revision = selected_revision.ok_or(StorageError::ToolNotReady)?;
        let status: Option<String> = row.try_get("status")?;
        if status.as_deref() != Some("ready") {
            return Err(StorageError::ToolNotReady);
        }
        Ok(ResolvedTool {
            tool_id: row.try_get("tool_id")?,
            revision: to_u64(selected_revision, "revision")?,
            artifact_digest: row
                .try_get::<Option<String>, _>("artifact_digest")?
                .ok_or_else(|| {
                    StorageError::Invariant("ready version has no artifact".to_owned())
                })?,
            input_format: parse_io_format(
                row.try_get::<Option<String>, _>("input_format")?
                    .ok_or_else(|| {
                        StorageError::Invariant("ready version has no input format".to_owned())
                    })?,
            )?,
            output_format: parse_io_format(
                row.try_get::<Option<String>, _>("output_format")?
                    .ok_or_else(|| {
                        StorageError::Invariant("ready version has no output format".to_owned())
                    })?,
            )?,
        })
    }

    pub async fn start_invocation(
        &self,
        invocation_id: Uuid,
        resolved: &ResolvedTool,
        stdin_size: u64,
    ) -> Result<(), StorageError> {
        sqlx::query(
            "INSERT INTO invocations
             (id, tool_id, revision, artifact_digest, status, stdin_size)
             VALUES ($1, $2, $3, $4, 'running', $5)",
        )
        .bind(invocation_id)
        .bind(resolved.tool_id)
        .bind(i64::try_from(resolved.revision).map_err(|_| StorageError::VersionNotFound)?)
        .bind(&resolved.artifact_digest)
        .bind(i64::try_from(stdin_size).unwrap_or(i64::MAX))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn finish_invocation(
        &self,
        invocation_id: Uuid,
        status: &str,
        exit_code: Option<i32>,
        duration_ms: u64,
        stdout_size: usize,
        stderr_size: usize,
    ) -> Result<(), StorageError> {
        sqlx::query(
            "UPDATE invocations
             SET status = $2, exit_code = $3, duration_ms = $4,
                 stdout_size = $5, stderr_size = $6, finished_at = now()
             WHERE id = $1",
        )
        .bind(invocation_id)
        .bind(status)
        .bind(exit_code)
        .bind(i64::try_from(duration_ms).unwrap_or(i64::MAX))
        .bind(i64::try_from(stdout_size).unwrap_or(i64::MAX))
        .bind(i64::try_from(stderr_size).unwrap_or(i64::MAX))
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

async fn ensure_job_owner(
    transaction: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    worker_id: &str,
) -> Result<(), StorageError> {
    let owned = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
            SELECT 1 FROM synthesis_jobs
            WHERE id = $1 AND worker_id = $2 AND status IN ('running', 'ready')
        )",
    )
    .bind(job_id)
    .bind(worker_id)
    .fetch_one(&mut **transaction)
    .await?;
    if !owned {
        return Err(StorageError::JobLeaseLost);
    }
    Ok(())
}

async fn append_agent_event_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    kind: &str,
    payload: &Value,
    trace_limit_bytes: i64,
) -> Result<(), StorageError> {
    let (payload, payload_size) = bounded_event_payload(payload)?;
    let trace_bytes: i64 = sqlx::query_scalar(
        "SELECT trace_bytes FROM synthesis_agent_runs WHERE job_id = $1 FOR UPDATE",
    )
    .bind(job_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or_else(|| StorageError::Invariant("agent run missing before event".to_owned()))?;
    let remaining = trace_limit_bytes.saturating_sub(trace_bytes);
    let (payload, payload_size) = if i64::from(payload_size) <= remaining {
        (payload, payload_size)
    } else {
        let omitted = serde_json::json!({
            "omitted": true,
            "reason": "agent trace byte budget exhausted"
        });
        let size = i32::try_from(serde_json::to_vec(&omitted)?.len()).unwrap_or(i32::MAX);
        if i64::from(size) > remaining {
            return Ok(());
        }
        (omitted, size)
    };
    sqlx::query(
        "INSERT INTO synthesis_agent_events (job_id, kind, payload, payload_size)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(job_id)
    .bind(kind)
    .bind(payload)
    .bind(payload_size)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "UPDATE synthesis_agent_runs
         SET trace_bytes = trace_bytes + $2, updated_at = now()
         WHERE job_id = $1",
    )
    .bind(job_id)
    .bind(i64::from(payload_size))
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn bounded_event_payload(payload: &Value) -> Result<(Value, i32), StorageError> {
    let encoded = serde_json::to_vec(payload)?;
    if encoded.len() <= AGENT_EVENT_PAYLOAD_LIMIT_BYTES {
        return Ok((
            payload.clone(),
            i32::try_from(encoded.len()).unwrap_or(i32::MAX),
        ));
    }
    let bounded = serde_json::json!({
        "omitted": true,
        "bytes": encoded.len(),
        "sha256": hex::encode(Sha256::digest(&encoded)),
        "reason": "event payload exceeded 16 KiB"
    });
    let size = i32::try_from(serde_json::to_vec(&bounded)?.len()).unwrap_or(i32::MAX);
    Ok((bounded, size))
}

#[derive(Clone, Debug)]
pub struct ClaimedSynthesisJob {
    pub job_id: Uuid,
    pub tool_id: Uuid,
    pub tool: String,
    pub revision: u64,
    pub intent: String,
    pub input_format: IoFormat,
    pub output_format: IoFormat,
    pub examples: Vec<ToolExample>,
    pub input_samples: Vec<String>,
    pub attempts: u32,
    pub agent_checkpoint: Option<AgentCheckpointRecord>,
}

#[derive(Clone, Debug)]
pub struct AgentCheckpointRecord {
    pub engine: String,
    pub engine_version: String,
    pub checkpoint: Value,
}

#[derive(Clone, Debug)]
pub struct NewJobInput {
    pub id: Uuid,
    pub agent_call_id: String,
    pub kind: JobInputKind,
    pub prompt: String,
    pub choices: Vec<JobInputChoice>,
    pub context: Value,
}

#[derive(Clone, Debug)]
pub struct PublishedArtifact {
    pub digest: String,
    pub relative_path: String,
    pub size_bytes: i64,
    pub manifest: Value,
    pub contract: Value,
    pub assumptions: Value,
    pub validation_summary: Value,
}

#[derive(Clone, Debug)]
pub struct ResolvedTool {
    pub tool_id: Uuid,
    pub revision: u64,
    pub artifact_digest: String,
    pub input_format: IoFormat,
    pub output_format: IoFormat,
}

pub fn registration_fingerprint(
    name: &ToolName,
    request: &RegistrationRequest,
) -> Result<String, StorageError> {
    let mut hasher = Sha256::new();
    hasher.update(name.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(serde_json::to_vec(request)?);
    Ok(hex::encode(hasher.finalize()))
}

fn next_offset(has_more: bool, offset: i64, limit: u32) -> Option<u64> {
    has_more
        .then(|| {
            u64::try_from(offset)
                .ok()
                .and_then(|offset| offset.checked_add(u64::from(limit)))
        })
        .flatten()
}

fn map_job(row: sqlx::postgres::PgRow) -> Result<JobResponse, StorageError> {
    let error_code: Option<String> = row.try_get("error_code")?;
    let error_message: Option<String> = row.try_get("error_message")?;
    let details: Option<Value> = row.try_get("details")?;
    let error = match (error_code, error_message) {
        (Some(code), Some(message)) => Some(JobError {
            code,
            message,
            details,
        }),
        _ => None,
    };
    let created_at: DateTime<Utc> = row.try_get("created_at")?;
    let updated_at: DateTime<Utc> = row.try_get("updated_at")?;
    let input_id: Option<Uuid> = row.try_get("input_id")?;
    let pending_input = if let Some(input_id) = input_id {
        let choices: Value = row.try_get("input_choices")?;
        let input_created_at: DateTime<Utc> = row.try_get("input_created_at")?;
        Some(PendingJobInput {
            id: input_id.to_string(),
            kind: parse_job_input_kind(row.try_get("input_kind")?)?,
            prompt: row.try_get("input_prompt")?,
            choices: serde_json::from_value(choices)?,
            context: row.try_get("input_context")?,
            created_at: input_created_at.to_rfc3339(),
        })
    } else {
        None
    };
    Ok(JobResponse {
        job_id: row.try_get::<Uuid, _>("id")?.to_string(),
        tool: row.try_get("name")?,
        revision: to_u64(row.try_get("revision")?, "revision")?,
        status: parse_job_status(row.try_get("job_status")?)?,
        stage: parse_job_stage(row.try_get("stage")?)?,
        version_status: parse_version_status(row.try_get("version_status")?)?,
        error,
        pending_input,
        created_at: created_at.to_rfc3339(),
        updated_at: updated_at.to_rfc3339(),
    })
}

fn parse_job_input_kind(value: String) -> Result<JobInputKind, StorageError> {
    JobInputKind::parse(&value)
        .ok_or_else(|| StorageError::Invariant(format!("unknown job input kind {value:?}")))
}

fn validate_job_input_answer(
    kind: JobInputKind,
    answer: &JobInputAnswer,
) -> Result<(), StorageError> {
    match (kind, answer) {
        (JobInputKind::Clarification, JobInputAnswer::Text { text })
            if !text.trim().is_empty() && text.len() <= 4096 =>
        {
            Ok(())
        }
        (JobInputKind::SourceApproval, JobInputAnswer::Approve) => Ok(()),
        (JobInputKind::SourceApproval, JobInputAnswer::Reject { reason })
            if reason.as_ref().is_none_or(|reason| reason.len() <= 4096) =>
        {
            Ok(())
        }
        (JobInputKind::ExampleCorrection, JobInputAnswer::Approve) => Ok(()),
        (JobInputKind::ExampleCorrection, JobInputAnswer::Reject { reason })
            if reason.as_ref().is_none_or(|reason| reason.len() <= 4096) =>
        {
            Ok(())
        }
        _ => Err(StorageError::InvalidJobInputAnswer),
    }
}

fn parse_version_status(value: String) -> Result<ToolVersionStatus, StorageError> {
    ToolVersionStatus::from_str(&value).map_err(|error| StorageError::Invariant(error.to_string()))
}

fn parse_job_status(value: String) -> Result<JobStatus, StorageError> {
    JobStatus::parse(&value)
        .ok_or_else(|| StorageError::Invariant(format!("unknown job status {value:?}")))
}

fn parse_job_stage(value: String) -> Result<JobStage, StorageError> {
    JobStage::parse(&value)
        .ok_or_else(|| StorageError::Invariant(format!("unknown job stage {value:?}")))
}

fn parse_io_format(value: String) -> Result<IoFormat, StorageError> {
    IoFormat::parse(&value)
        .ok_or_else(|| StorageError::Invariant(format!("unknown I/O format {value:?}")))
}

fn to_u64(value: i64, field: &str) -> Result<u64, StorageError> {
    u64::try_from(value)
        .map_err(|_| StorageError::Invariant(format!("{field} must not be negative")))
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("tool not found")]
    ToolNotFound,

    #[error("tool version not found")]
    VersionNotFound,

    #[error("tool version in status {0:?} cannot be revoked")]
    VersionNotRevocable(String),

    #[error("synthesis job not found")]
    JobNotFound,

    #[error("synthesis job in status {0:?} cannot be cancelled")]
    JobNotCancellable(String),

    #[error("cancellation reason must contain 1-4096 bytes")]
    InvalidCancellationReason,

    #[error("pending synthesis job input was not found")]
    JobInputNotFound,

    #[error("answer does not match the pending synthesis job input")]
    InvalidJobInputAnswer,

    #[error("HTTP capability approval was not found or is already revoked")]
    HttpCapabilityNotFound,

    #[error("revocation reason must contain 1-4096 bytes")]
    InvalidRevocationReason,

    #[error("idempotency key was already used for a different request")]
    IdempotencyConflict,

    #[error("tool has no ready version")]
    ToolNotReady,

    #[error("tool version has no published artifact")]
    ArtifactNotFound,

    #[error("synthesis job lease was lost")]
    JobLeaseLost,

    #[error("registry invariant violated: {0}")]
    Invariant(String),

    #[error(transparent)]
    Database(#[from] sqlx::Error),

    #[error(transparent)]
    Migration(#[from] sqlx::migrate::MigrateError),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use jit_protocol::ToolExample;

    #[test]
    fn registration_fingerprint_is_stable_and_payload_sensitive() {
        let name = ToolName::from_str("slugify").unwrap();
        let mut request = RegistrationRequest {
            intent: "make a slug".to_owned(),
            input_format: IoFormat::Text,
            output_format: IoFormat::Text,
            examples: vec![ToolExample {
                input: "Hello".to_owned(),
                output: "hello".to_owned(),
            }],
            input_samples: vec!["Hello from stdin\n".to_owned()],
        };
        let first = registration_fingerprint(&name, &request).unwrap();
        assert_eq!(first, registration_fingerprint(&name, &request).unwrap());
        request.intent.push('!');
        assert_ne!(first, registration_fingerprint(&name, &request).unwrap());
    }
}
