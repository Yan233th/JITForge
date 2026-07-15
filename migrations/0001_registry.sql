CREATE TABLE tools (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    latest_revision BIGINT NOT NULL DEFAULT 0 CHECK (latest_revision >= 0),
    stable_revision BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE tool_versions (
    tool_id UUID NOT NULL REFERENCES tools(id) ON DELETE CASCADE,
    revision BIGINT NOT NULL CHECK (revision > 0),
    description TEXT NOT NULL,
    input_format TEXT NOT NULL CHECK (input_format IN ('text', 'json')),
    output_format TEXT NOT NULL CHECK (output_format IN ('text', 'json')),
    examples JSONB NOT NULL DEFAULT '[]'::jsonb,
    status TEXT NOT NULL CHECK (status IN (
        'draft', 'contract_ready', 'synthesizing', 'building', 'validating',
        'ready', 'rejected', 'deprecated', 'revoked'
    )),
    contract JSONB,
    assumptions JSONB NOT NULL DEFAULT '[]'::jsonb,
    artifact_digest TEXT,
    validation_summary JSONB,
    error_code TEXT,
    error_message TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tool_id, revision)
);

ALTER TABLE tools
    ADD CONSTRAINT tools_stable_version_fk
    FOREIGN KEY (id, stable_revision)
    REFERENCES tool_versions(tool_id, revision)
    DEFERRABLE INITIALLY DEFERRED;

CREATE TABLE synthesis_jobs (
    id UUID PRIMARY KEY,
    tool_id UUID NOT NULL,
    revision BIGINT NOT NULL,
    idempotency_key TEXT NOT NULL UNIQUE,
    request_fingerprint TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('queued', 'running', 'ready', 'rejected')),
    stage TEXT NOT NULL CHECK (stage IN (
        'queued', 'contract', 'synthesizing', 'building', 'validating',
        'repairing', 'complete'
    )),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    available_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    lease_until TIMESTAMPTZ,
    worker_id TEXT,
    error_code TEXT,
    error_message TEXT,
    details JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (tool_id, revision) REFERENCES tool_versions(tool_id, revision) ON DELETE CASCADE
);

CREATE INDEX synthesis_jobs_claim_idx
    ON synthesis_jobs(status, available_at, lease_until, created_at);

CREATE TABLE artifacts (
    digest TEXT PRIMARY KEY,
    relative_path TEXT NOT NULL,
    size_bytes BIGINT NOT NULL CHECK (size_bytes >= 0),
    manifest JSONB NOT NULL,
    validation_summary JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE invocations (
    id UUID PRIMARY KEY,
    tool_id UUID NOT NULL,
    revision BIGINT NOT NULL,
    artifact_digest TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'timed_out')),
    exit_code INTEGER,
    duration_ms BIGINT,
    stdin_size BIGINT NOT NULL CHECK (stdin_size >= 0),
    stdout_size BIGINT,
    stderr_size BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    FOREIGN KEY (tool_id, revision) REFERENCES tool_versions(tool_id, revision),
    FOREIGN KEY (artifact_digest) REFERENCES artifacts(digest)
);

CREATE TABLE worker_heartbeats (
    worker_id TEXT PRIMARY KEY,
    version TEXT NOT NULL,
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
