-- Add optional job snapshot and parsing configuration to watchers.

ALTER TABLE watchers
    ADD COLUMN IF NOT EXISTS last_jobs_json JSONB,
    ADD COLUMN IF NOT EXISTS job_selector TEXT;

