-- Idempotency table for Stripe webhook events.
-- Stripe retries failed webhooks for up to 72 hours; this prevents
-- the same event from being processed more than once.

CREATE TABLE IF NOT EXISTS stripe_events (
    stripe_event_id  TEXT        PRIMARY KEY,
    event_type       TEXT        NOT NULL,
    processed_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    payload          JSONB       NOT NULL
);
