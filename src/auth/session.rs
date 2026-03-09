//! Session token generation and database operations.
//!
//! Security model:
//! - A 32-byte random token is generated and sent to the client as a cookie.
//! - Only the SHA-256 hash of that token is stored in the database.
//! - A stolen DB dump cannot be used to hijack sessions.

use anyhow::Result;
use chrono::Utc;
use rand::Rng;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::User;

/// Generate a new random session token and its SHA-256 hash.
///
/// Returns `(raw_token, token_hash)`. The raw token goes into the cookie;
/// the hash is stored in `sessions.token_hash`.
pub fn new_token() -> (String, String) {
    let raw: [u8; 32] = rand::thread_rng().gen();
    let token: String = raw.iter().map(|b| format!("{:02x}", b)).collect();
    let hash = hash_token(&token);
    (token, hash)
}

/// Compute the SHA-256 hex digest of a token string.
pub fn hash_token(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    h.finalize().iter().map(|b| format!("{:02x}", b)).collect()
}

/// Persist a new session for `user_id` and return the raw token for the cookie.
pub async fn create_session(db: &PgPool, user_id: Uuid) -> Result<String> {
    let (token, token_hash) = new_token();
    sqlx::query(
        "INSERT INTO sessions (id, user_id, token_hash) VALUES ($1, $2, $3)",
    )
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(&token_hash)
    .execute(db)
    .await?;
    Ok(token)
}

/// Look up the user associated with a raw session token.
///
/// Returns `None` if the session is missing, expired, or the token is invalid.
pub async fn get_user_by_token(db: &PgPool, token: &str) -> Result<Option<User>> {
    let token_hash = hash_token(token);
    let user = sqlx::query_as::<_, User>(
        r#"
        SELECT u.id, u.email, u.password_hash, u.email_verified, u.tier,
               u.stripe_customer_id, u.stripe_subscription_id,
               u.subscription_status, u.is_admin, u.created_at, u.updated_at
        FROM sessions s
        JOIN users u ON u.id = s.user_id
        WHERE s.token_hash = $1
          AND s.expires_at > now()
        "#,
    )
    .bind(token_hash)
    .fetch_optional(db)
    .await?;
    Ok(user)
}

/// Delete a specific session (single-device logout).
pub async fn revoke_session(db: &PgPool, token: &str) -> Result<()> {
    let token_hash = hash_token(token);
    sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
        .bind(token_hash)
        .execute(db)
        .await?;
    Ok(())
}

/// Delete all sessions for a user (all-device logout, e.g. after password reset).
pub async fn revoke_all_sessions(db: &PgPool, user_id: Uuid) -> Result<()> {
    sqlx::query("DELETE FROM sessions WHERE user_id = $1")
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Create a time-limited one-time token for email verification or password reset.
///
/// `kind` is one of `"email_verification"` or `"password_reset"`.
/// Returns the raw token to be embedded in a link.
pub async fn create_email_token(
    db: &PgPool,
    user_id: Uuid,
    kind: &str,
    expires_in_minutes: i64,
) -> Result<String> {
    let (token, token_hash) = new_token();
    let expires_at = Utc::now() + chrono::Duration::minutes(expires_in_minutes);
    sqlx::query(
        "INSERT INTO email_tokens (id, user_id, token_hash, kind, expires_at)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(&token_hash)
    .bind(kind)
    .bind(expires_at)
    .execute(db)
    .await?;
    Ok(token)
}

/// Validate and consume a one-time email token.

///
/// Returns the `user_id` if the token is valid, unused, and not expired.
/// Marks the token as used atomically so it cannot be replayed.
pub async fn consume_email_token(
    db: &PgPool,
    token: &str,
    expected_kind: &str,
) -> Result<Option<Uuid>> {
    let token_hash = hash_token(token);
    let row = sqlx::query_as::<_, (Uuid,)>(
        r#"
        UPDATE email_tokens
        SET used_at = now()
        WHERE token_hash = $1
          AND kind = $2
          AND expires_at > now()
          AND used_at IS NULL
        RETURNING user_id
        "#,
    )
    .bind(token_hash)
    .bind(expected_kind)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(id,)| id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_64_hex_chars() {
        let (token, _) = new_token();
        assert_eq!(token.len(), 64, "32 bytes encodes to 64 hex chars");
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn tokens_are_unique() {
        let (t1, _) = new_token();
        let (t2, _) = new_token();
        assert_ne!(t1, t2, "consecutive tokens must not collide");
    }

    #[test]
    fn hash_is_deterministic() {
        let (token, expected_hash) = new_token();
        assert_eq!(hash_token(&token), expected_hash);
        assert_eq!(hash_token(&token), expected_hash); // stable on repeated calls
    }

    #[test]
    fn hash_is_64_hex_chars() {
        let (_, hash) = new_token();
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn different_tokens_produce_different_hashes() {
        let (t1, h1) = new_token();
        let (t2, h2) = new_token();
        assert_ne!(h1, h2, "distinct tokens must not hash to the same value");
        // Sanity: confirm it's the hash of the token, not of t2
        assert_eq!(hash_token(&t1), h1);
        assert_eq!(hash_token(&t2), h2);
    }

    #[test]
    fn raw_token_differs_from_its_hash() {
        // The cookie value (raw token) must never equal what's stored in the DB.
        let (token, hash) = new_token();
        assert_ne!(token, hash);
    }
}
