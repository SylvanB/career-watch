-- Initial schema for career-watch

CREATE TABLE IF NOT EXISTS watchers (
    id UUID PRIMARY KEY,
    company_name TEXT NOT NULL,
    url TEXT NOT NULL,
    email TEXT NOT NULL,
    last_hash TEXT,
    last_checked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    active BOOLEAN NOT NULL DEFAULT TRUE
);

CREATE INDEX IF NOT EXISTS idx_watchers_active_created_at
    ON watchers (active, created_at DESC);

