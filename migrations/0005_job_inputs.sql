ALTER TABLE synthesis_jobs
    DROP CONSTRAINT synthesis_jobs_status_check,
    ADD CONSTRAINT synthesis_jobs_status_check
        CHECK (status IN ('queued', 'running', 'awaiting_input', 'ready', 'rejected')),
    DROP CONSTRAINT synthesis_jobs_stage_check,
    ADD CONSTRAINT synthesis_jobs_stage_check
        CHECK (stage IN (
            'queued', 'contract', 'synthesizing', 'building', 'validating',
            'repairing', 'awaiting_input', 'complete'
        ));

CREATE TABLE synthesis_job_inputs (
    id UUID PRIMARY KEY,
    job_id UUID NOT NULL REFERENCES synthesis_jobs(id) ON DELETE CASCADE,
    agent_call_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('clarification', 'source_approval')),
    prompt TEXT NOT NULL,
    choices JSONB NOT NULL DEFAULT '[]'::jsonb,
    context JSONB NOT NULL DEFAULT '{}'::jsonb,
    resume_stage TEXT NOT NULL CHECK (resume_stage IN (
        'contract', 'synthesizing', 'building', 'validating', 'repairing'
    )),
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'answered')),
    answer JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    answered_at TIMESTAMPTZ,
    UNIQUE (job_id, agent_call_id)
);

CREATE UNIQUE INDEX synthesis_job_inputs_one_pending_idx
    ON synthesis_job_inputs(job_id)
    WHERE status = 'pending';
