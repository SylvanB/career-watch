-- Link watchers back to the discovery catalog so we can show
-- which companies a user is already watching on the Discover page.

ALTER TABLE watchers
    ADD COLUMN IF NOT EXISTS company_id UUID REFERENCES companies(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_watchers_company_id ON watchers(company_id);
