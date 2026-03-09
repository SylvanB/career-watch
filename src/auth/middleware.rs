//! Axum middleware that validates the session cookie and injects `AuthUser`
//! into request extensions.

use axum::{extract::State, http::StatusCode, middleware::Next, response::Response};
use axum::extract::Request;

use crate::auth::session::get_user_by_token;
use crate::models::AuthUser;
use crate::AppState;

pub const SESSION_COOKIE: &str = "cw_session";

/// Extract the raw session token value from the `Cookie` header.
fn extract_session_token(req: &Request) -> Option<String> {
    let cookie_str = req.headers().get("cookie")?.to_str().ok()?;
    let prefix = format!("{}=", SESSION_COOKIE);
    cookie_str
        .split(';')
        .map(str::trim)
        .find(|p| p.starts_with(&prefix))
        .map(|p| p[prefix.len()..].to_string())
}

/// Middleware that requires a valid, unexpired session cookie.
///
/// On success, injects [`AuthUser`] into request extensions.
/// Returns `401 Unauthorized` if the cookie is missing or the session is invalid.
pub async fn require_auth(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Log what cookies (if any) the API is actually receiving from the proxy.
    let cookie_hdr = req.headers().get("cookie").and_then(|v| v.to_str().ok());
    tracing::debug!(
        path  = %req.uri().path(),
        has_cookie_header = cookie_hdr.is_some(),
        cookie_names = cookie_hdr
            .map(|c| c.split(';')
                .map(|p| p.split('=').next().unwrap_or("?").trim().to_string())
                .collect::<Vec<_>>()
                .join(", "))
            .unwrap_or_default()
            .as_str(),
        "require_auth"
    );

    let token = extract_session_token(&req).ok_or(StatusCode::UNAUTHORIZED)?;

    let user = get_user_by_token(&state.db, &token)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;

    req.extensions_mut().insert(AuthUser::from(user));
    Ok(next.run(req).await)
}

/// Middleware that requires the `Authorization: Bearer {ADMIN_SECRET}` header.
pub async fn require_admin(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let auth_header = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let expected = format!("Bearer {}", state.config.admin_secret);
    if auth_header != expected {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(next.run(req).await)
}
