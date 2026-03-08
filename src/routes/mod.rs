//! Top-level HTTP routing for the Career Watch API.
//!
//! Exposes health checks and CRUD endpoints for watcher resources.

use axum::routing::{delete, get};
use axum::Router;

use crate::routes::{
    health::health,
    watches::{create_watch, delete_watch, list_watches},
};
use crate::AppState;

pub mod health;
pub mod watches;

/// Construct the root router for the HTTP API.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/healthz", get(health))
        .route("/api/watches", get(list_watches).post(create_watch))
        .route("/api/watches/:id", delete(delete_watch))
}

