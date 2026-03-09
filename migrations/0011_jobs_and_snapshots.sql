-- Hiring analytics: individual job records and daily rollup snapshots.
--
-- jobs            → one row per unique role per company; upserted on each worker pass
-- company_hiring_snapshots → pre-computed daily rollup for fast time-series reads

CREATE TABLE IF NOT EXISTS jobs (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    company_id          UUID        NOT NULL REFERENCES companies(id) ON DELETE CASCADE,
    -- Stable dedup key: SHA-256 hex of "<company_id>:<lower(title)>:<lower(location)>"
    job_hash            TEXT        NOT NULL,
    title               TEXT        NOT NULL,
    raw_department      TEXT,
    -- Normalised categories for rollups
    department_category TEXT        NOT NULL DEFAULT 'other',
    raw_location        TEXT,
    location_region     TEXT        NOT NULL DEFAULT 'unknown',
    is_remote           BOOLEAN     NOT NULL DEFAULT FALSE,
    -- Source URL for the individual job posting (when available)
    url                 TEXT,
    first_seen_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 'active' while the posting exists; 'filled' when it disappears from the listing
    status              TEXT        NOT NULL DEFAULT 'active',
    filled_at           TIMESTAMPTZ,
    -- Proxy for hiring speed: days between first_seen_at and filled_at
    time_to_fill_days   INTEGER,
    UNIQUE (company_id, job_hash)
);

CREATE INDEX idx_jobs_company_status     ON jobs (company_id, status);
CREATE INDEX idx_jobs_company_first_seen ON jobs (company_id, first_seen_at DESC);
CREATE INDEX idx_jobs_dept_active        ON jobs (company_id, department_category) WHERE status = 'active';
CREATE INDEX idx_jobs_region_active      ON jobs (company_id, location_region)     WHERE status = 'active';

-- ── Daily snapshots ────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS company_hiring_snapshots (
    id              UUID    PRIMARY KEY DEFAULT gen_random_uuid(),
    company_id      UUID    NOT NULL REFERENCES companies(id) ON DELETE CASCADE,
    snapshot_date   DATE    NOT NULL,
    -- Totals
    active_count    INTEGER NOT NULL DEFAULT 0,
    new_count       INTEGER NOT NULL DEFAULT 0,    -- first_seen on this date
    filled_count    INTEGER NOT NULL DEFAULT 0,    -- filled on this date
    -- Department breakdown (active jobs)
    eng_count       INTEGER NOT NULL DEFAULT 0,
    product_count   INTEGER NOT NULL DEFAULT 0,
    sales_count     INTEGER NOT NULL DEFAULT 0,
    marketing_count INTEGER NOT NULL DEFAULT 0,
    design_count    INTEGER NOT NULL DEFAULT 0,
    data_count      INTEGER NOT NULL DEFAULT 0,
    ops_count       INTEGER NOT NULL DEFAULT 0,
    people_count    INTEGER NOT NULL DEFAULT 0,
    other_count     INTEGER NOT NULL DEFAULT 0,
    -- Geo breakdown (active jobs)
    remote_count    INTEGER NOT NULL DEFAULT 0,
    us_count        INTEGER NOT NULL DEFAULT 0,
    europe_count    INTEGER NOT NULL DEFAULT 0,
    apac_count      INTEGER NOT NULL DEFAULT 0,
    other_geo_count INTEGER NOT NULL DEFAULT 0,
    UNIQUE (company_id, snapshot_date)
);

CREATE INDEX idx_snapshots_company_date ON company_hiring_snapshots (company_id, snapshot_date DESC);
