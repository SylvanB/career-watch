-- Discovery pipeline: tracks auto-discovered companies and their career pages.
--
-- The `companies` table is the source of truth for the discovery pipeline.
-- It is separate from `watchers`, which are user-managed subscriptions.
-- Discovered companies can be promoted to watchers manually.

CREATE TABLE IF NOT EXISTS companies (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    name            TEXT        NOT NULL,
    domain          TEXT        NOT NULL UNIQUE,
    source          TEXT        NOT NULL,           -- e.g. 'github_org:rust-lang', 'static_list'
    career_url      TEXT,                           -- resolved career page URL after probing
    platform        TEXT,                           -- greenhouse | lever | workday | ashby | smartrecruiters | unknown
    platform_id     TEXT,                           -- company slug on the ATS platform, if detected
    status          TEXT        NOT NULL DEFAULT 'pending',  -- pending | active | failed
    probe_error     TEXT,                           -- last error message when status = 'failed'
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_probed_at  TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_companies_status        ON companies(status);
CREATE INDEX IF NOT EXISTS idx_companies_last_probed   ON companies(last_probed_at ASC NULLS FIRST);
