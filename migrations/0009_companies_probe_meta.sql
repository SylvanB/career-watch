-- Store confidence score and discovery source alongside probe results
-- so the catalog can surface higher-confidence results first.

ALTER TABLE companies
    ADD COLUMN IF NOT EXISTS probe_confidence SMALLINT,
    ADD COLUMN IF NOT EXISTS probe_source     TEXT;

CREATE INDEX IF NOT EXISTS idx_companies_confidence
    ON companies (probe_confidence DESC NULLS LAST)
    WHERE status = 'active';
