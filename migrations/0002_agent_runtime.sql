CREATE TABLE synthesis_agent_runs (
    job_id UUID PRIMARY KEY REFERENCES synthesis_jobs(id) ON DELETE CASCADE,
    engine TEXT NOT NULL,
    engine_version TEXT NOT NULL,
    checkpoint JSONB NOT NULL,
    trace_bytes BIGINT NOT NULL DEFAULT 0 CHECK (trace_bytes >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE synthesis_agent_events (
    id BIGSERIAL PRIMARY KEY,
    job_id UUID NOT NULL REFERENCES synthesis_jobs(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    payload JSONB NOT NULL,
    payload_size INTEGER NOT NULL CHECK (payload_size >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX synthesis_agent_events_job_idx
    ON synthesis_agent_events(job_id, id);

CREATE TABLE synthesis_agent_trace_archives (
    job_id UUID PRIMARY KEY REFERENCES synthesis_jobs(id) ON DELETE CASCADE,
    encoding TEXT NOT NULL DEFAULT 'postgres-jsonb-toast',
    event_count INTEGER NOT NULL CHECK (event_count >= 0),
    events JSONB NOT NULL,
    archived_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
