-- Notification archive: one row per alert sent to a user.
-- Persists independently of watchers (ON DELETE SET NULL) so the
-- history survives if a watcher is removed.

CREATE TABLE notifications (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id      UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    watcher_id   UUID        REFERENCES watchers(id) ON DELETE SET NULL,
    company_name TEXT        NOT NULL,
    career_url   TEXT        NOT NULL,
    -- Human-readable summary, e.g. "3 jobs added, 1 removed"
    summary      TEXT        NOT NULL,
    added_jobs   JSONB       NOT NULL DEFAULT '[]',
    removed_jobs JSONB       NOT NULL DEFAULT '[]',
    read_at      TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_notifications_user_created
    ON notifications (user_id, created_at DESC);

CREATE INDEX idx_notifications_unread
    ON notifications (user_id)
    WHERE read_at IS NULL;
