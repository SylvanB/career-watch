-- Store the auto-detected CSS selector for job listings on each discovered
-- career page. When a user watches a company from the catalog, this selector
-- is copied to their watcher so they get job-level diffs automatically.

ALTER TABLE companies
    ADD COLUMN IF NOT EXISTS job_selector TEXT;
