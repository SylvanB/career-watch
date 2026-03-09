-- Users, sessions, and email token tables for SaaS auth + billing.

CREATE TYPE subscription_tier AS ENUM ('free', 'pro', 'business');

CREATE TABLE users (
    id                     UUID             PRIMARY KEY DEFAULT gen_random_uuid(),
    email                  TEXT             NOT NULL UNIQUE,
    password_hash          TEXT             NOT NULL,
    email_verified         BOOLEAN          NOT NULL DEFAULT FALSE,
    tier                   subscription_tier NOT NULL DEFAULT 'free',
    stripe_customer_id     TEXT             UNIQUE,
    stripe_subscription_id TEXT             UNIQUE,
    subscription_status    TEXT             NOT NULL DEFAULT 'inactive',
    is_admin               BOOLEAN          NOT NULL DEFAULT FALSE,
    created_at             TIMESTAMPTZ      NOT NULL DEFAULT now(),
    updated_at             TIMESTAMPTZ      NOT NULL DEFAULT now()
);

-- Short-lived sessions identified by an opaque token.
-- Only the SHA-256 hash of the token is stored; the raw value lives in the cookie.
CREATE TABLE sessions (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id      UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash   TEXT        NOT NULL UNIQUE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at   TIMESTAMPTZ NOT NULL DEFAULT now() + interval '30 days'
);

-- One-time tokens for email verification and password reset.
CREATE TABLE email_tokens (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash  TEXT        NOT NULL UNIQUE,
    kind        TEXT        NOT NULL,  -- 'email_verification' | 'password_reset'
    expires_at  TIMESTAMPTZ NOT NULL,
    used_at     TIMESTAMPTZ            -- set when consumed; NULL = still valid
);

CREATE INDEX idx_sessions_token_hash   ON sessions(token_hash);
CREATE INDEX idx_sessions_expires_at   ON sessions(expires_at);
CREATE INDEX idx_email_tokens_hash     ON email_tokens(token_hash);
