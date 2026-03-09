//! Notification archive endpoints.
//!
//! GET  /api/notifications          — list notifications (newest first, paginated)
//! POST /api/notifications/read-all — mark all as read

use axum::{
    extract::{Query, State},
    http::StatusCode,
    Extension, Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::{AuthUser, JobPosting};
use crate::AppState;

#[derive(Deserialize)]
pub struct ListParams {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
    /// When true, return only unread notifications.
    #[serde(default)]
    pub unread_only: bool,
}

fn default_limit() -> i64 { 50 }

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct NotificationRow {
    pub id: Uuid,
    pub watcher_id: Option<Uuid>,
    pub company_name: String,
    pub career_url: String,
    pub summary: String,
    pub added_jobs: sqlx::types::Json<Vec<JobPosting>>,
    pub removed_jobs: sqlx::types::Json<Vec<JobPosting>>,
    pub read_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

pub async fn list_notifications(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Query(params): Query<ListParams>,
) -> Result<Json<Vec<NotificationRow>>, StatusCode> {
    let limit = params.limit.clamp(1, 200);

    let rows = if params.unread_only {
        sqlx::query_as::<_, NotificationRow>(
            r#"
            SELECT id, watcher_id, company_name, career_url, summary,
                   added_jobs, removed_jobs, read_at, created_at
            FROM notifications
            WHERE user_id = $1 AND read_at IS NULL
            ORDER BY created_at DESC
            LIMIT $2 OFFSET $3
            "#,
        )
        .bind(user.id)
        .bind(limit)
        .bind(params.offset)
        .fetch_all(&state.db)
        .await
    } else {
        sqlx::query_as::<_, NotificationRow>(
            r#"
            SELECT id, watcher_id, company_name, career_url, summary,
                   added_jobs, removed_jobs, read_at, created_at
            FROM notifications
            WHERE user_id = $1
            ORDER BY created_at DESC
            LIMIT $2 OFFSET $3
            "#,
        )
        .bind(user.id)
        .bind(limit)
        .bind(params.offset)
        .fetch_all(&state.db)
        .await
    };

    rows.map(Json).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn mark_all_read(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
) -> StatusCode {
    let result = sqlx::query(
        "UPDATE notifications SET read_at = now()
         WHERE user_id = $1 AND read_at IS NULL",
    )
    .bind(user.id)
    .execute(&state.db)
    .await;

    match result {
        Ok(_) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
