//! Data models for watchers, users, and API payloads.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use sqlx::FromRow;
use uuid::Uuid;

// ── Subscription tier ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, sqlx::Type, Serialize, Deserialize)]
#[sqlx(type_name = "subscription_tier", rename_all = "lowercase")]
pub enum SubscriptionTier {
    Free,
    Pro,
    Business,
}

impl SubscriptionTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            SubscriptionTier::Free => "free",
            SubscriptionTier::Pro => "pro",
            SubscriptionTier::Business => "business",
        }
    }

    pub fn max_watchers(&self) -> i64 {
        match self {
            SubscriptionTier::Free => 3,
            SubscriptionTier::Pro => 25,
            SubscriptionTier::Business => 200,
        }
    }

    pub fn check_interval_minutes(&self) -> i64 {
        match self {
            SubscriptionTier::Free => 60 * 24, // daily
            SubscriptionTier::Pro => 60,        // hourly
            SubscriptionTier::Business => 15,   // 15-minute
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            SubscriptionTier::Free => "Free",
            SubscriptionTier::Pro => "Pro",
            SubscriptionTier::Business => "Business",
        }
    }

    pub fn monthly_price_usd(&self) -> Option<u32> {
        match self {
            SubscriptionTier::Free => None,
            SubscriptionTier::Pro => Some(9),
            SubscriptionTier::Business => Some(29),
        }
    }
}

// ── User ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, FromRow)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub password_hash: String,
    pub email_verified: bool,
    pub tier: SubscriptionTier,
    pub stripe_customer_id: Option<String>,
    pub stripe_subscription_id: Option<String>,
    pub subscription_status: String,
    pub is_admin: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request-scoped auth context injected by the auth middleware.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: Uuid,
    pub email: String,
    pub email_verified: bool,
    pub tier: SubscriptionTier,
    pub stripe_customer_id: Option<String>,
    pub is_admin: bool,
}

impl From<User> for AuthUser {
    fn from(u: User) -> Self {
        Self {
            id: u.id,
            email: u.email,
            email_verified: u.email_verified,
            tier: u.tier,
            stripe_customer_id: u.stripe_customer_id,
            is_admin: u.is_admin,
        }
    }
}

// ── Watchers ──────────────────────────────────────────────────────────────────

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
    pub user_id: Option<Uuid>,
}

/// Public representation of a watcher returned from the HTTP API.
#[derive(Debug, Serialize)]
pub struct Watch {
    pub id: Uuid,
    pub company_name: String,
    pub url: String,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub active: bool,
    pub job_selector: Option<String>,
}

impl From<WatchRow> for Watch {
    fn from(row: WatchRow) -> Self {
        Self {
            id: row.id,
            company_name: row.company_name,
            url: row.url,
            last_checked_at: row.last_checked_at,
            created_at: row.created_at,
            active: row.active,
            job_selector: row.job_selector,
        }
    }
}

/// Request body for creating a new watcher.
///
/// `email` is omitted — notifications go to the authenticated user's account email.
#[derive(Debug, Deserialize)]
pub struct CreateWatchRequest {
    pub company_name: String,
    pub url: String,
    /// Optional CSS selector for structured job-level change detection.
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
