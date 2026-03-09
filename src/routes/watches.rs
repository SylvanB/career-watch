//! HTTP handlers for creating, listing, and deleting watchers.
//!
//! All operations are scoped to the authenticated user. Watcher creation
//! is gated on the user's tier limit.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use serde::Deserialize;
use uuid::Uuid;
use url::Url;

use crate::models::{AuthUser, CreateWatchRequest, Watch, WatchRow};
use crate::AppState;

/// List all active watchers for the current user.
pub async fn list_watches(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
) -> Result<Json<Vec<Watch>>, StatusCode> {
    let rows: Vec<WatchRow> = sqlx::query_as::<_, WatchRow>(
        r#"
        SELECT id, company_name, url, email, last_hash, last_checked_at,
               created_at, active, last_jobs_json, job_selector, user_id, company_id
        FROM watchers
        WHERE active = true
          AND user_id = $1
        ORDER BY created_at DESC
        "#,
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(rows.into_iter().map(Watch::from).collect()))
}

/// Create a new watcher for the current user.
///
/// Enforces the tier watcher limit. Notifications go to the user's account email.
pub async fn create_watch(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(payload): Json<CreateWatchRequest>,
) -> Result<(StatusCode, Json<Watch>), (StatusCode, Json<serde_json::Value>)> {
    if Url::parse(&payload.url).is_err() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid URL"})),
        ));
    }

    if !user.email_verified {
        return Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "email address must be verified before creating watchers"})),
        ));
    }

    let max = user.tier.max_watchers();
    let current: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM watchers WHERE user_id = $1 AND active = true",
    )
    .bind(user.id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "internal error"})),
        )
    })?;

    if current >= max {
        return Err((
            StatusCode::PAYMENT_REQUIRED,
            Json(serde_json::json!({
                "error": "watcher limit reached for your plan",
                "current": current,
                "limit": max,
                "tier": user.tier.as_str(),
            })),
        ));
    }

    let row: WatchRow = sqlx::query_as::<_, WatchRow>(
        r#"
        INSERT INTO watchers (id, company_name, url, email, job_selector, user_id)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, company_name, url, email, last_hash, last_checked_at,
                  created_at, active, last_jobs_json, job_selector, user_id
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(&payload.company_name)
    .bind(&payload.url)
    .bind(&user.email)  // notifications go to the account email
    .bind(&payload.job_selector)
    .bind(user.id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "internal error"})),
        )
    })?;

    // If approaching the limit (>= 80%), send an upgrade nudge
    let after = current + 1;
    if after >= (max * 4 / 5) && after < max && user.tier.monthly_price_usd().is_none() {
        let upgrade_url = format!("{}/pricing", state.config.app_base_url);
        let _ = state
            .email
            .send_upgrade_prompt_email(&user.email, after, max, &upgrade_url)
            .await;
    }

    Ok((StatusCode::CREATED, Json(row.into())))
}

/// Update a watcher's URL and/or CSS selector.
///
/// Resets `last_hash` and `last_jobs_json` so the next worker run
/// establishes a fresh baseline rather than sending a spurious diff.
pub async fn update_watch(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateWatchRequest>,
) -> Result<Json<Watch>, (StatusCode, Json<serde_json::Value>)> {
    fn err(s: StatusCode, msg: &str) -> (StatusCode, Json<serde_json::Value>) {
        (s, Json(serde_json::json!({"error": msg})))
    }

    // Validate URL if provided
    if let Some(ref url) = payload.url {
        if Url::parse(url).is_err() {
            return Err(err(StatusCode::BAD_REQUEST, "invalid URL"));
        }
    }

    let row: WatchRow = sqlx::query_as::<_, WatchRow>(
        r#"
        UPDATE watchers
        SET url           = COALESCE($1, url),
            job_selector  = $2,
            -- reset baseline so the worker doesn't fire a spurious diff
            last_hash     = NULL,
            last_jobs_json = NULL,
            last_checked_at = NULL
        WHERE id = $3 AND user_id = $4 AND active = true
        RETURNING id, company_name, url, email, last_hash, last_checked_at,
                  created_at, active, last_jobs_json, job_selector, user_id, company_id
        "#,
    )
    .bind(payload.url.as_deref())
    .bind(payload.job_selector.as_deref())   // NULL clears the selector
    .bind(id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "internal error"))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "watcher not found"))?;

    Ok(Json(row.into()))
}

#[derive(Deserialize)]
pub struct UpdateWatchRequest {
    /// New URL. `None` = keep existing.
    pub url: Option<String>,
    /// New CSS selector. `None` = clear (reverts to hash-only mode).
    /// Pass an explicit `null` in JSON to clear, or omit to keep existing —
    /// but since JSON `null` and missing field both deserialise to `None`,
    /// the API treats `None` as "clear". Use `PATCH` only when you intend to change.
    pub job_selector: Option<String>,
}

/// Soft-delete a watcher. Only the owning user may delete their own watchers.
pub async fn delete_watch(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, StatusCode> {
    let rows = sqlx::query(
        "UPDATE watchers SET active = false WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user.id)
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
