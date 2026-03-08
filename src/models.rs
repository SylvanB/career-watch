//! Data models for watchers and API payloads.
//!
//! `WatchRow` represents the database row, while `Watch` is the API-facing
//! representation returned from HTTP handlers.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use sqlx::FromRow;
use uuid::Uuid;

/// Row shape for the `watchers` table in Postgres.
#[derive(Debug, FromRow)]
pub struct WatchRow {
    pub id: Uuid,
    pub company_name: String,
    pub url: String,
    pub email: String,
    pub last_hash: Option<String>,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub active: bool,
    pub last_jobs_json: Option<Json<Vec<JobPosting>>>,
    pub job_selector: Option<String>,
}

/// Public representation of a watcher returned from the HTTP API.
#[derive(Debug, Serialize)]
pub struct Watch {
    pub id: Uuid,
    pub company_name: String,
    pub url: String,
    pub email: String,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub active: bool,
}

impl From<WatchRow> for Watch {
    fn from(row: WatchRow) -> Self {
        Self {
            id: row.id,
            company_name: row.company_name,
            url: row.url,
            email: row.email,
            last_checked_at: row.last_checked_at,
            created_at: row.created_at,
            active: row.active,
        }
    }
}

/// Request body for creating a new watcher via the HTTP API.
#[derive(Debug, Deserialize)]
pub struct CreateWatchRequest {
    pub company_name: String,
    pub url: String,
    pub email: String,
    /// Optional CSS selector used to extract individual job postings.
    ///
    /// When set, the worker will attempt to parse structured job listings
    /// instead of only hashing the full page.
    pub job_selector: Option<String>,
}

/// A single job posting extracted from a career page.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub struct JobPosting {
    pub title: String,
    pub company: String,
    pub location: Option<String>,
    /// Free-form text representing when this job was posted,
    /// for example "14 minutes ago" or "2025-01-12".
    pub posted_text: Option<String>,
    /// Optional link to the job detail page.
    pub url: Option<String>,
}

