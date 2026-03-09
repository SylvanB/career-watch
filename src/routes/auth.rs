//! Authentication route handlers.
//!
//! POST /auth/register       — create account
//! POST /auth/login          — start session
//! POST /auth/logout         — end session
//! POST /auth/verify-email   — confirm email address
//! POST /auth/forgot-password — request password reset link
//! POST /auth/reset-password — set new password via reset token

use axum::{
    extract::State,
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::{
    middleware::SESSION_COOKIE,
    password::{hash_password, verify_password},
    session::{
        consume_email_token, create_email_token, create_session, revoke_all_sessions,
        revoke_session,
    },
};
use crate::AppState;

// ── Request / response types ──────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct VerifyEmailRequest {
    pub token: String,
}

#[derive(Deserialize)]
pub struct ForgotPasswordRequest {
    pub email: String,
}

#[derive(Deserialize)]
pub struct ResetPasswordRequest {
    pub token: String,
    pub new_password: String,
}

#[derive(Serialize)]
pub struct AuthResponse {
    pub user_id: Uuid,
    pub email: String,
    pub email_verified: bool,
}

type ApiError = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiError {
    (status, Json(serde_json::json!({"error": msg})))
}

// ── Cookie helpers ────────────────────────────────────────────────────────────

/// Attach a `Set-Cookie` session header to any response.
fn with_session_cookie(inner: impl IntoResponse, token: &str, secure: bool) -> Response {
    let cookie = format!(
        "{}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}",
        SESSION_COOKIE,
        token,
        30 * 24 * 3600,
        if secure { "; Secure" } else { "" }
    );
    let mut res = inner.into_response();
    if let Ok(v) = HeaderValue::from_str(&cookie) {
        res.headers_mut().insert(header::SET_COOKIE, v);
    }
    res
}

/// Attach an expiring `Set-Cookie` header to clear the session cookie.
fn with_clear_cookie(inner: impl IntoResponse) -> Response {
    let cookie = format!("{}=; Path=/; Max-Age=0", SESSION_COOKIE);
    let mut res = inner.into_response();
    if let Ok(v) = HeaderValue::from_str(&cookie) {
        res.headers_mut().insert(header::SET_COOKIE, v);
    }
    res
}

/// Extract the session token from the request `Cookie` header.
fn read_session_cookie(headers: &axum::http::HeaderMap) -> Option<String> {
    let cookie_str = headers.get("cookie")?.to_str().ok()?;
    let prefix = format!("{}=", SESSION_COOKIE);
    cookie_str
        .split(';')
        .map(str::trim)
        .find(|p| p.starts_with(&prefix))
        .map(|p| p[prefix.len()..].to_string())
}

// ── Handlers ──────────────────────────────────────────────────────────────────

pub async fn register(
    State(state): State<AppState>,
    Json(payload): Json<RegisterRequest>,
) -> Result<Response, ApiError> {
    if !payload.email.contains('@') {
        return Err(err(StatusCode::BAD_REQUEST, "invalid email address"));
    }
    if payload.password.len() < 8 {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "password must be at least 8 characters",
        ));
    }

    let password_hash = hash_password(&payload.password)
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "internal error"))?;

    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, email, password_hash) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(&payload.email)
        .bind(&password_hash)
        .execute(&state.db)
        .await
        .map_err(|e| {
            let s = e.to_string();
            if s.contains("unique") || s.contains("duplicate") {
                err(StatusCode::CONFLICT, "email already registered")
            } else {
                err(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
            }
        })?;

    // Create Stripe customer (best-effort)
    if !state.config.stripe_secret_key.is_empty() {
        match state
            .stripe
            .create_customer(&payload.email, &user_id.to_string())
            .await
        {
            Ok(cid) => {
                let _ = sqlx::query("UPDATE users SET stripe_customer_id = $1 WHERE id = $2")
                    .bind(&cid)
                    .bind(user_id)
                    .execute(&state.db)
                    .await;
            }
            Err(e) => tracing::warn!("Stripe customer creation failed: {e}"),
        }
    }

    // Send verification email (best-effort)
    if let Ok(token) = create_email_token(&state.db, user_id, "email_verification", 24 * 60).await {
        let url = format!("{}/verify-email?token={}", state.config.app_base_url, token);
        let _ = state
            .email
            .send_verification_email(&payload.email, &url)
            .await;
    }

    let session = create_session(&state.db, user_id)
        .await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "internal error"))?;

    let _ = state.email.send_welcome_email(&payload.email).await;

    let body = Json(AuthResponse {
        user_id,
        email: payload.email,
        email_verified: false,
    });
    Ok(with_session_cookie(
        (StatusCode::CREATED, body),
        &session,
        state.config.cookie_secure,
    ))
}

pub async fn login(
    State(state): State<AppState>,
    Json(payload): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    let row = sqlx::query_as::<_, (Uuid, String, String, bool)>(
        "SELECT id, email, password_hash, email_verified FROM users WHERE email = $1",
    )
    .bind(&payload.email)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "internal error"))?
    .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "invalid credentials"))?;

    let (user_id, email, password_hash, email_verified) = row;

    if !verify_password(&payload.password, &password_hash) {
        return Err(err(StatusCode::UNAUTHORIZED, "invalid credentials"));
    }

    let session = create_session(&state.db, user_id)
        .await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "internal error"))?;

    let body = Json(AuthResponse {
        user_id,
        email,
        email_verified,
    });
    Ok(with_session_cookie(
        body,
        &session,
        state.config.cookie_secure,
    ))
}

pub async fn logout(State(state): State<AppState>, req: axum::extract::Request) -> Response {
    if let Some(token) = read_session_cookie(req.headers()) {
        let _ = revoke_session(&state.db, &token).await;
    }
    with_clear_cookie(StatusCode::NO_CONTENT)
}

pub async fn verify_email(
    State(state): State<AppState>,
    Json(payload): Json<VerifyEmailRequest>,
) -> Result<StatusCode, ApiError> {
    let user_id = consume_email_token(&state.db, &payload.token, "email_verification")
        .await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "internal error"))?
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "invalid or expired token"))?;

    sqlx::query("UPDATE users SET email_verified = true, updated_at = now() WHERE id = $1")
        .bind(user_id)
        .execute(&state.db)
        .await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "internal error"))?;

    Ok(StatusCode::NO_CONTENT)
}

/// Always returns 204 regardless of whether the email exists (prevents enumeration).
pub async fn forgot_password(
    State(state): State<AppState>,
    Json(payload): Json<ForgotPasswordRequest>,
) -> StatusCode {
    if let Ok(Some((user_id, email))) =
        sqlx::query_as::<_, (Uuid, String)>("SELECT id, email FROM users WHERE email = $1")
            .bind(&payload.email)
            .fetch_optional(&state.db)
            .await
    {
        if let Ok(token) = create_email_token(&state.db, user_id, "password_reset", 60).await {
            let url = format!(
                "{}/reset-password?token={}",
                state.config.app_base_url, token
            );
            let _ = state.email.send_password_reset_email(&email, &url).await;
        }
    }
    StatusCode::NO_CONTENT
}

pub async fn reset_password(
    State(state): State<AppState>,
    Json(payload): Json<ResetPasswordRequest>,
) -> Result<StatusCode, ApiError> {
    if payload.new_password.len() < 8 {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "password must be at least 8 characters",
        ));
    }

    let user_id = consume_email_token(&state.db, &payload.token, "password_reset")
        .await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "internal error"))?
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "invalid or expired token"))?;

    let new_hash = hash_password(&payload.new_password)
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "internal error"))?;

    sqlx::query("UPDATE users SET password_hash = $1, updated_at = now() WHERE id = $2")
        .bind(&new_hash)
        .bind(user_id)
        .execute(&state.db)
        .await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "internal error"))?;

    let _ = revoke_all_sessions(&state.db, user_id).await;

    Ok(StatusCode::NO_CONTENT)
}
