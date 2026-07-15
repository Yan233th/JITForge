ALTER TABLE synthesis_jobs
    ADD COLUMN input_samples JSONB NOT NULL DEFAULT '[]'::jsonb;
