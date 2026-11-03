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
            SubscriptionTier::Pro => 60,       // hourly
            SubscriptionTier::Business => 15,  // 15-minute
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
    pub company_id: Option<Uuid>,
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
    /// Set when this watcher was created from the company catalog.
    /// Used by the frontend to link to the signals dashboard.
    pub company_id: Option<Uuid>,
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
            company_id: row.company_id,
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
///
/// Identity is based solely on `title` + `company`. All other fields are
/// metadata that may change between scrapes and must not affect equality
/// or hashing — otherwise every scrape would report every job as changed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobPosting {
    pub title: String,
    pub company: String,
    pub location: Option<String>,
    /// Normalised department category (e.g. "engineering", "sales").
    /// Populated by LLM extraction; falls back to keyword inference.
    pub department: Option<String>,
    pub posted_text: Option<String>,
    pub url: Option<String>,
}

impl PartialEq for JobPosting {
    fn eq(&self, other: &Self) -> bool {
        self.title == other.title && self.company == other.company
    }
}

impl Eq for JobPosting {}

impl std::hash::Hash for JobPosting {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.title.hash(state);
        self.company.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn job(title: &str) -> JobPosting {
        JobPosting {
            title: title.to_string(),
            company: "Acme".to_string(),
            location: None,
            department: None,
            posted_text: None,
            url: None,
        }
    }

    fn job_full(title: &str, location: &str, posted: &str, url: &str) -> JobPosting {
        JobPosting {
            title: title.to_string(),
            company: "Acme".to_string(),
            location: Some(location.to_string()),
            department: None,
            posted_text: Some(posted.to_string()),
            url: Some(url.to_string()),
        }
    }

    // ── JobPosting identity ───────────────────────────────────────────────────

    #[test]
    fn eq_ignores_metadata() {
        // Same title+company but different location, posted_text, url → equal
        let a = job_full("Engineer", "Remote", "1 hour ago", "https://example.com/1");
        let b = job_full("Engineer", "New York", "just now", "https://example.com/2");
        assert_eq!(a, b);
    }

    #[test]
    fn neq_different_title() {
        assert_ne!(job("Engineer"), job("Designer"));
    }

    #[test]
    fn neq_different_company() {
        let a = JobPosting {
            title: "Engineer".into(),
            company: "Acme".into(),
            location: None,
            department: None,
            posted_text: None,
            url: None,
        };
        let b = JobPosting {
            title: "Engineer".into(),
            company: "Globex".into(),
            location: None,
            department: None,
            posted_text: None,
            url: None,
        };
        assert_ne!(a, b);
    }

    #[test]
    fn hash_consistent_with_eq() {
        // If a == b, inserting b into a set containing a must be a no-op
        let a = job_full("Engineer", "Remote", "1h ago", "https://example.com/old");
        let b = job_full("Engineer", "NYC", "just now", "https://example.com/new");
        let mut set = HashSet::new();
        set.insert(a);
        assert!(!set.insert(b), "equal job should not be re-inserted");
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn hash_set_dedup_on_title_company() {
        // Simulate two scrapes of the same page: posted_text changes each time.
        let scrape1 = vec![
            job_full("Senior Engineer", "Remote", "1h ago", "https://co.com/1"),
            job_full("Product Manager", "NYC", "2h ago", "https://co.com/2"),
        ];
        let scrape2 = vec![
            job_full("Senior Engineer", "Remote", "2h ago", "https://co.com/1"),
            job_full("Product Manager", "NYC", "3h ago", "https://co.com/2"),
            job_full("Data Scientist", "Remote", "just now", "https://co.com/3"),
        ];

        let old: HashSet<_> = scrape1.into_iter().collect();
        let new: HashSet<_> = scrape2.into_iter().collect();

        let added: Vec<_> = new.difference(&old).collect();
        let removed: Vec<_> = old.difference(&new).collect();

        assert_eq!(added.len(), 1, "only Data Scientist should be added");
        assert_eq!(added[0].title, "Data Scientist");
        assert_eq!(removed.len(), 0, "no jobs should be removed");
    }

    // ── SubscriptionTier limits ───────────────────────────────────────────────

    #[test]
    fn tier_watcher_limits() {
        assert_eq!(SubscriptionTier::Free.max_watchers(), 3);
        assert_eq!(SubscriptionTier::Pro.max_watchers(), 25);
        assert_eq!(SubscriptionTier::Business.max_watchers(), 200);
    }

    #[test]
    fn tier_check_intervals() {
        assert_eq!(SubscriptionTier::Free.check_interval_minutes(), 24 * 60);
        assert_eq!(SubscriptionTier::Pro.check_interval_minutes(), 60);
        assert_eq!(SubscriptionTier::Business.check_interval_minutes(), 15);
    }

    #[test]
    fn tier_pricing() {
        assert!(SubscriptionTier::Free.monthly_price_usd().is_none());
        assert_eq!(SubscriptionTier::Pro.monthly_price_usd(), Some(9));
        assert_eq!(SubscriptionTier::Business.monthly_price_usd(), Some(29));
    }

    #[test]
    fn tier_ordering_by_limit() {
        assert!(SubscriptionTier::Free.max_watchers() < SubscriptionTier::Pro.max_watchers());
        assert!(SubscriptionTier::Pro.max_watchers() < SubscriptionTier::Business.max_watchers());
    }
}
