//! HTTP routing for the Career Watch API.
//!
//! Routes are split into three groups with different middleware stacks:
//!
//! - **Public**    — no auth required (`/healthz`, `/auth/*`, `/billing/webhook`)
//! - **Protected** — requires session cookie (`/api/*`)
//! - **Admin**     — requires `Authorization: Bearer {ADMIN_SECRET}` (`/admin/*`)

use std::time::Duration;

use axum::{
    middleware,
    routing::{delete, get, patch, post},
    Router,
};
use tower_http::trace::TraceLayer;

use crate::auth::middleware::{require_admin, require_auth};
use crate::rate_limit::{rate_limit, RateLimiter};
use crate::routes::{
    admin::{get_user, list_users, patch_user_tier},
    auth::{forgot_password, login, logout, register, reset_password, verify_email},
    billing::{create_checkout, create_portal, stripe_webhook},
    health::health,
    me::me,
    notifications::{list_notifications, mark_all_read},
    watches::{create_watch, delete_watch, list_watches},
};
use crate::AppState;

pub mod admin;
pub mod auth;
pub mod billing;
pub mod health;
pub mod me;
pub mod notifications;
pub mod watches;

/// Construct the root router with all route groups and middleware applied.
pub fn router(state: AppState) -> Router {
    // Rate limiter for auth endpoints: 10 requests per minute per IP.
    // Shared across all auth routes so login + register + forgot-password
    // together are bounded.
    let auth_limiter = RateLimiter::new(10, Duration::from_secs(60));

    let public = Router::new()
        .route("/healthz", get(health))
        // Auth (rate-limited)
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/verify-email", post(verify_email))
        .route("/auth/forgot-password", post(forgot_password))
        .route("/auth/reset-password", post(reset_password))
        .layer(middleware::from_fn_with_state(auth_limiter, rate_limit))
        // Stripe webhook — no session auth, uses Stripe signature verification
        .route("/billing/webhook", post(stripe_webhook));

    let protected = Router::new()
        .route("/api/me", get(me))
        .route("/api/notifications", get(list_notifications))
        .route("/api/notifications/read-all", post(mark_all_read))
        .route("/api/watches", get(list_watches).post(create_watch))
        .route("/api/watches/:id", delete(delete_watch))
        .route("/billing/checkout", post(create_checkout))
        .route("/billing/portal", post(create_portal))
        .layer(middleware::from_fn_with_state(state.clone(), require_auth));

    let admin = Router::new()
        .route("/admin/users", get(list_users))
        .route("/admin/users/:id", get(get_user))
        .route("/admin/users/:id/tier", patch(patch_user_tier))
        .layer(middleware::from_fn_with_state(state.clone(), require_admin));

    Router::new()
        .merge(public)
        .merge(protected)
        .merge(admin)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
