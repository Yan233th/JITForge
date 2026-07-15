ALTER TABLE tool_versions
    ADD COLUMN requested_intent TEXT;

UPDATE tool_versions
SET requested_intent = description;

ALTER TABLE tool_versions
    ALTER COLUMN requested_intent SET NOT NULL;
