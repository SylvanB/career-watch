//! Internal admin route handlers.
//!
//! Protected by `require_admin` middleware (Bearer token auth).
//!
//! GET  /admin/users              — paginated user list
//! GET  /admin/users/:id          — user detail with watcher count
//! PATCH /admin/users/:id/tier    — override a user's subscription tier

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::SubscriptionTier;
use crate::AppState;

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct AdminUserRow {
    pub id: Uuid,
    pub email: String,
    pub email_verified: bool,
    pub tier: SubscriptionTier,
    pub subscription_status: String,
    pub is_admin: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct AdminUserDetail {
    #[serde(flatten)]
    pub user: AdminUserRow,
    pub watcher_count: i64,
    pub stripe_customer_id: Option<String>,
}

// ── Query params ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct PaginationParams {
    #[serde(default = "default_page")]
    pub page: i64,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_page() -> i64 {
    1
}
fn default_limit() -> i64 {
    50
}

// ── Handlers ──────────────────────────────────────────────────────────────────

pub async fn list_users(
    State(state): State<AppState>,
    Query(params): Query<PaginationParams>,
) -> Result<Json<Vec<AdminUserRow>>, StatusCode> {
    let limit = params.limit.min(200);
    let offset = ((params.page - 1).max(0)) * limit;

    let rows = sqlx::query_as::<_, AdminUserRow>(
        r#"
        SELECT id, email, email_verified, tier, subscription_status, is_admin, created_at
        FROM users
        ORDER BY created_at DESC
        LIMIT $1 OFFSET $2
        "#,
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(rows))
}

pub async fn get_user(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<AdminUserDetail>, StatusCode> {
    let user = sqlx::query_as::<_, AdminUserRow>(
        "SELECT id, email, email_verified, tier, subscription_status, is_admin, created_at
         FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::NOT_FOUND)?;

    let watcher_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM watchers WHERE user_id = $1 AND active = true")
            .bind(user_id)
            .fetch_one(&state.db)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let stripe_customer_id: Option<String> =
        sqlx::query_scalar("SELECT stripe_customer_id FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_one(&state.db)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(AdminUserDetail {
        user,
        watcher_count,
        stripe_customer_id,
    }))
}

#[derive(Deserialize)]
pub struct PatchTierRequest {
    pub tier: SubscriptionTier,
}

pub async fn patch_user_tier(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
    Json(payload): Json<PatchTierRequest>,
) -> Result<StatusCode, StatusCode> {
    let rows = sqlx::query("UPDATE users SET tier = $1, updated_at = now() WHERE id = $2")
        .bind(&payload.tier)
        .bind(user_id)
        .execute(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .rows_affected();

    if rows == 0 {
        Err(StatusCode::NOT_FOUND)
    } else {
        Ok(StatusCode::NO_CONTENT)
    }
}
