ALTER TABLE synthesis_job_inputs
    DROP CONSTRAINT synthesis_job_inputs_kind_check,
    ADD CONSTRAINT synthesis_job_inputs_kind_check
        CHECK (kind IN ('clarification', 'source_approval', 'example_correction'));
