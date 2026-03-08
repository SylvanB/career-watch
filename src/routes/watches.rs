//! HTTP handlers for creating, listing, and deleting watchers.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;
use url::Url;

use crate::models::{CreateWatchRequest, Watch, WatchRow};
use crate::AppState;

/// List all active watchers ordered by creation time (newest first).
pub async fn list_watches(
    State(AppState { db, .. }): State<AppState>,
) -> Result<Json<Vec<Watch>>, StatusCode> {
    let rows: Vec<WatchRow> = sqlx::query_as::<_, WatchRow>(
        r#"
        SELECT id, company_name, url, email, last_hash, last_checked_at, created_at, active, last_jobs_json, job_selector
        FROM watchers
        WHERE active = true
        ORDER BY created_at DESC
        "#,
    )
    .fetch_all(&db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(rows.into_iter().map(Watch::from).collect()))
}

/// Create a new watcher for a given company career page.
///
/// The URL is validated syntactically; deeper validation (e.g. reachability)
/// is left to the worker when it attempts to scrape the page.
pub async fn create_watch(
    State(AppState { db, .. }): State<AppState>,
    Json(payload): Json<CreateWatchRequest>,
) -> Result<(StatusCode, Json<Watch>), StatusCode> {
    if Url::parse(&payload.url).is_err() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let row: WatchRow = sqlx::query_as::<_, WatchRow>(
        r#"
        INSERT INTO watchers (id, company_name, url, email, job_selector)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, company_name, url, email, last_hash, last_checked_at, created_at, active, last_jobs_json, job_selector
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(&payload.company_name)
    .bind(&payload.url)
    .bind(&payload.email)
    .bind(&payload.job_selector)
    .fetch_one(&db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok((StatusCode::CREATED, Json(row.into())))
}

/// Soft-delete a watcher by marking it inactive.
pub async fn delete_watch(
    State(AppState { db, .. }): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, StatusCode> {
    sqlx::query(
        r#"
        UPDATE watchers
        SET active = false
        WHERE id = $1
        "#,
    )
    .bind(id)
    .execute(&db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::NO_CONTENT)
}

