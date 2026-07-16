CREATE TABLE http_capability_approvals (
    capability_hash TEXT PRIMARY KEY,
    capability JSONB NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'revoked')),
    approved_by_job UUID REFERENCES synthesis_jobs(id) ON DELETE SET NULL,
    approved_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ
);

CREATE INDEX http_capability_approvals_status_idx
    ON http_capability_approvals(status, approved_at DESC);
