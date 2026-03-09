-- Scope watchers to a user account for tenant isolation.
--
-- user_id is nullable initially so existing rows are preserved.
-- The application layer enforces that new rows always have a user_id.

ALTER TABLE watchers
    ADD COLUMN IF NOT EXISTS user_id UUID REFERENCES users(id) ON DELETE CASCADE;

CREATE INDEX IF NOT EXISTS idx_watchers_user_id ON watchers(user_id);
