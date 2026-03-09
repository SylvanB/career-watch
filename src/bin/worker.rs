//! Background worker for change detection and email notifications.
//!
//! Run once per invocation (cron-friendly). Fetches all active watchers
//! whose check interval has elapsed (based on the owning user's tier),
//! scrapes each page, diffs job listings, and sends email notifications
//! when changes are detected.
//!
//! ## Check intervals by tier
//!
//! | Tier     | Interval  |
//! |----------|-----------|
//! | Free     | 24 hours  |
//! | Pro      | 1 hour    |
//! | Business | 15 min    |
//!
//! Run this worker from cron every **15 minutes**. The SQL WHERE clause
//! filters which watchers are actually due, so higher tiers get more frequent
//! checks automatically.
//!
//! ```cron
//! */15 * * * * /path/to/career-watch-worker
//! ```

use career_watch::config::AppConfig;
use career_watch::db::create_pool;
use career_watch::email::EmailClient;
use career_watch::models::{JobPosting, SubscriptionTier};
use career_watch::scraper::PageScraper;
use chrono::Utc;
use sqlx::types::Json;
use sqlx::PgPool;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .finish()
        .init();

    let config = AppConfig::from_env()?;
    let pool = create_pool(&config.database_url).await?;
    sqlx::migrate!().run(&pool).await?;

    let email = EmailClient::from_config(&config)?;
    let scraper = PageScraper::new();

    run_once(&pool, &email, &scraper).await?;

    Ok(())
}

/// Extended watch row that includes the owning user's tier, id, and email.
#[derive(sqlx::FromRow)]
struct WatchWithUser {
    id: Uuid,
    user_id: Option<Uuid>,
    company_name: String,
    url: String,
    // notification email (stored on the watcher, copied from user.email at creation)
    email: String,
    last_hash: Option<String>,
    last_jobs_json: Option<Json<Vec<JobPosting>>>,
    job_selector: Option<String>,
    #[allow(dead_code)] // selected by SQL for the WHERE filter, not used in Rust
    tier: SubscriptionTier,
}

async fn run_once(
    pool: &PgPool,
    email: &EmailClient,
    scraper: &PageScraper,
) -> anyhow::Result<()> {
    // Fetch only watchers that are due based on their owner's tier interval.
    // Running the cron every 15 minutes and filtering here gives each tier
    // the correct check frequency without separate cron entries.
    let watches: Vec<WatchWithUser> = sqlx::query_as::<_, WatchWithUser>(
        r#"
        SELECT w.id, w.user_id, w.company_name, w.url, w.email,
               w.last_hash, w.last_jobs_json, w.job_selector,
               u.tier
        FROM watchers w
        JOIN users u ON u.id = w.user_id
        WHERE w.active = true
          AND (
              (u.tier = 'free'     AND (w.last_checked_at IS NULL OR w.last_checked_at < now() - interval '24 hours'))
           OR (u.tier = 'pro'      AND (w.last_checked_at IS NULL OR w.last_checked_at < now() - interval '1 hour'))
           OR (u.tier = 'business' AND (w.last_checked_at IS NULL OR w.last_checked_at < now() - interval '15 minutes'))
          )
        "#,
    )
    .fetch_all(pool)
    .await?;

    tracing::info!(count = watches.len(), "checking watchers");

    for watch in watches {
        let now = Utc::now();

        let (new_hash, added_jobs, removed_jobs, new_jobs_snapshot) =
            match &watch.job_selector {
                Some(selector) if !selector.trim().is_empty() => {
                    let old_jobs: Vec<JobPosting> = watch
                        .last_jobs_json
                        .as_ref()
                        .map(|Json(jobs)| jobs.clone())
                        .unwrap_or_default();

                    let new_jobs = match scraper
                        .fetch_jobs(&watch.url, selector, &watch.company_name)
                        .await
                    {
                        Ok(jobs) => jobs,
                        Err(err) => {
                            tracing::warn!(
                                id = %watch.id,
                                url = watch.url,
                                "failed to fetch jobs: {err}"
                            );
                            continue;
                        }
                    };

                    let (added, removed) = diff_jobs(&old_jobs, &new_jobs);

                    let hash = match scraper.fetch_hash(&watch.url).await {
                        Ok(h) => h,
                        Err(err) => {
                            tracing::warn!(id = %watch.id, "failed to fetch hash: {err}");
                            continue;
                        }
                    };

                    (hash, added, removed, Some(new_jobs))
                }
                _ => {
                    let new_hash = match scraper.fetch_hash(&watch.url).await {
                        Ok(h) => h,
                        Err(err) => {
                            tracing::warn!(
                                id = %watch.id,
                                url = watch.url,
                                "failed to fetch page: {err}"
                            );
                            continue;
                        }
                    };
                    (new_hash, Vec::new(), Vec::new(), None)
                }
            };

        let should_notify = match watch.last_hash {
            None => false,
            Some(ref existing) => existing != &new_hash,
        } || !added_jobs.is_empty()
          || !removed_jobs.is_empty();

        if should_notify {
            if !watch.email.is_empty() {
                if let Err(err) = email
                    .send_change_email(
                        &watch.email,
                        &watch.company_name,
                        &watch.url,
                        &added_jobs,
                        &removed_jobs,
                    )
                    .await
                {
                    tracing::warn!(id = %watch.id, "failed to send email: {err}");
                }
            }

            // Archive the notification regardless of email success
            if let Some(uid) = watch.user_id {
                let summary = notification_summary(&added_jobs, &removed_jobs);
                if let Err(err) = sqlx::query(
                    r#"INSERT INTO notifications
                       (id, user_id, watcher_id, company_name, career_url,
                        summary, added_jobs, removed_jobs)
                       VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"#,
                )
                .bind(Uuid::new_v4())
                .bind(uid)
                .bind(watch.id)
                .bind(&watch.company_name)
                .bind(&watch.url)
                .bind(&summary)
                .bind(Json(&added_jobs))
                .bind(Json(&removed_jobs))
                .execute(pool)
                .await
                {
                    tracing::warn!(id = %watch.id, "failed to insert notification: {err}");
                }
            }
        }

        let jobs_json: Option<Json<Vec<JobPosting>>> = new_jobs_snapshot.map(Json);

        if let Err(err) = sqlx::query(
            "UPDATE watchers SET last_hash = $1, last_checked_at = $2, last_jobs_json = $3
             WHERE id = $4",
        )
        .bind(&new_hash)
        .bind(now)
        .bind(jobs_json)
        .bind(watch.id)
        .execute(pool)
        .await
        {
            tracing::warn!(id = %watch.id, "failed to update watcher after scrape: {err}");
        }
    }

    Ok(())
}

fn notification_summary(added: &[JobPosting], removed: &[JobPosting]) -> String {
    match (added.len(), removed.len()) {
        (0, 0) => "Career page updated".to_string(),
        (a, 0) => format!("{} new job{} added", a, if a == 1 { "" } else { "s" }),
        (0, r) => format!("{} job{} removed", r, if r == 1 { "" } else { "s" }),
        (a, r) => format!(
            "{} job{} added, {} removed",
            a,
            if a == 1 { "" } else { "s" },
            r
        ),
    }
}

fn diff_jobs(old: &[JobPosting], new: &[JobPosting]) -> (Vec<JobPosting>, Vec<JobPosting>) {
    use std::collections::HashSet;
    let old_set: HashSet<_> = old.iter().cloned().collect();
    let new_set: HashSet<_> = new.iter().cloned().collect();
    let added = new_set.difference(&old_set).cloned().collect();
    let removed = old_set.difference(&new_set).cloned().collect();
    (added, removed)
}

#[cfg(test)]
mod tests {
    use super::{diff_jobs, notification_summary};
    use career_watch::models::JobPosting;

    fn job(title: &str) -> JobPosting {
        JobPosting {
            title: title.to_string(),
            company: "Acme".to_string(),
            location: None,
            posted_text: None,
            url: None,
        }
    }

    // ── diff_jobs ─────────────────────────────────────────────────────────────

    #[test]
    fn diff_detects_added_and_removed() {
        let old = vec![job("A"), job("B")];
        let new = vec![job("B"), job("C")];
        let (added, removed) = diff_jobs(&old, &new);
        assert_eq!(added, vec![job("C")]);
        assert_eq!(removed, vec![job("A")]);
    }

    #[test]
    fn diff_empty_lists_produce_no_changes() {
        let (added, removed) = diff_jobs(&[], &[]);
        assert!(added.is_empty());
        assert!(removed.is_empty());
    }

    #[test]
    fn diff_all_new_jobs_added() {
        let (added, removed) = diff_jobs(&[], &[job("X"), job("Y")]);
        assert_eq!(added.len(), 2);
        assert!(removed.is_empty());
    }

    #[test]
    fn diff_all_jobs_removed() {
        let (added, removed) = diff_jobs(&[job("X"), job("Y")], &[]);
        assert!(added.is_empty());
        assert_eq!(removed.len(), 2);
    }

    #[test]
    fn diff_identical_lists_produce_no_changes() {
        let jobs = vec![job("A"), job("B"), job("C")];
        let (added, removed) = diff_jobs(&jobs, &jobs.clone());
        assert!(added.is_empty());
        assert!(removed.is_empty());
    }

    #[test]
    fn diff_ignores_metadata_changes() {
        // Same title+company, different posted_text — should NOT appear as a change
        let old = vec![JobPosting {
            title: "Engineer".into(), company: "Acme".into(),
            location: None, posted_text: Some("1h ago".into()), url: None,
        }];
        let new = vec![JobPosting {
            title: "Engineer".into(), company: "Acme".into(),
            location: None, posted_text: Some("2h ago".into()), url: None,
        }];
        let (added, removed) = diff_jobs(&old, &new);
        assert!(added.is_empty(), "metadata-only change should not appear as added");
        assert!(removed.is_empty(), "metadata-only change should not appear as removed");
    }

    // ── notification_summary ──────────────────────────────────────────────────

    #[test]
    fn summary_no_structured_jobs() {
        assert_eq!(notification_summary(&[], &[]), "Career page updated");
    }

    #[test]
    fn summary_single_job_added() {
        assert_eq!(notification_summary(&[job("A")], &[]), "1 new job added");
    }

    #[test]
    fn summary_multiple_jobs_added() {
        assert_eq!(notification_summary(&[job("A"), job("B")], &[]), "2 new jobs added");
    }

    #[test]
    fn summary_single_job_removed() {
        assert_eq!(notification_summary(&[], &[job("A")]), "1 job removed");
    }

    #[test]
    fn summary_multiple_jobs_removed() {
        assert_eq!(notification_summary(&[], &[job("A"), job("B")]), "2 jobs removed");
    }

    #[test]
    fn summary_added_and_removed() {
        assert_eq!(
            notification_summary(&[job("A")], &[job("B")]),
            "1 job added, 1 removed"
        );
        assert_eq!(
            notification_summary(&[job("A"), job("B")], &[job("C")]),
            "2 jobs added, 1 removed"
        );
    }
}
