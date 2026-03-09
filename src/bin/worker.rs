//! Background worker for change detection, email notifications, and job tracking.
//!
//! Run once per cron invocation. Processes all active watchers due for a check
//! based on their owner's tier, then updates the hiring analytics tables.
//!
//! ## Check intervals by tier
//!
//! | Tier     | Interval  |
//! |----------|-----------|
//! | Free     | 24 hours  |
//! | Pro      | 1 hour    |
//! | Business | 15 min    |
//!
//! Run cron every **15 minutes**; the SQL WHERE clause handles per-tier filtering.
//!
//! ```cron
//! */15 * * * * /path/to/career-watch-worker
//! ```

use career_watch::analytics::categorize::{categorize_department, categorize_location};
use career_watch::analytics::metrics::upsert_daily_snapshot;
use career_watch::config::AppConfig;
use career_watch::discovery::llm_selector::extract_jobs_via_llm;
use career_watch::db::create_pool;
use career_watch::email::EmailClient;
use career_watch::models::{JobPosting, SubscriptionTier};
use career_watch::scraper::PageScraper;
use chrono::Utc;
use sha2::{Digest, Sha256};
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

    run_once(&pool, &email, &scraper, &config.ollama_host, &config.ollama_extraction_model).await?;

    Ok(())
}

/// Extended watch row including analytics-relevant fields.
#[derive(sqlx::FromRow)]
struct WatchWithUser {
    id: Uuid,
    user_id: Option<Uuid>,
    company_id: Option<Uuid>,
    company_name: String,
    url: String,
    email: String,
    last_hash: Option<String>,
    last_jobs_json: Option<Json<Vec<JobPosting>>>,
    job_selector: Option<String>,
    /// Two-level selectors from LLM detection — clean title without location noise.
    title_selector: Option<String>,
    location_selector: Option<String>,
    #[allow(dead_code)]
    tier: SubscriptionTier,
}

async fn run_once(
    pool: &PgPool,
    email: &EmailClient,
    scraper: &PageScraper,
    ollama_host: &str,
    ollama_model: &str,
) -> anyhow::Result<()> {
    let watches: Vec<WatchWithUser> = sqlx::query_as::<_, WatchWithUser>(
        r#"
        SELECT w.id, w.user_id, w.company_id, w.company_name, w.url, w.email,
               w.last_hash, w.last_jobs_json, w.job_selector,
               w.title_selector, w.location_selector,
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

        tracing::info!(
            company  = watch.company_name,
            has_selector  = watch.job_selector.is_some(),
            has_company_id = watch.company_id.is_some(),
            "processing watcher"
        );

        // Fetch the page HTML once — used for both hash and job extraction.
        let html = match scraper.fetch_raw_html(&watch.url).await {
            Ok(h) => h,
            Err(err) => {
                tracing::warn!(id = %watch.id, url = watch.url, "failed to fetch page: {err}");
                continue;
            }
        };

        let new_hash = career_watch::scraper::hash_body(&html);

        // Determine which extraction mode to use.
        // Priority: LLM (when Ollama available) → CSS selectors → hash-only.
        let has_selector = watch.job_selector.as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);

        let use_llm = !ollama_host.is_empty();

        let (added_jobs, removed_jobs, new_jobs_snapshot) = if use_llm || has_selector {
            let old_jobs: Vec<JobPosting> = watch
                .last_jobs_json
                .as_ref()
                .map(|Json(jobs)| jobs.clone())
                .unwrap_or_default();

            // Try LLM extraction first; fall back to CSS selectors.
            let new_jobs_opt: Option<Vec<JobPosting>> = if use_llm {
                match extract_jobs_via_llm(&html, &watch.url, &watch.company_name, ollama_host, ollama_model).await {
                    Ok(jobs) => jobs,
                    Err(e) => {
                        tracing::warn!(company = watch.company_name, "LLM extraction error: {e}");
                        None
                    }
                }
            } else {
                None
            };

            // If LLM returned nothing (not configured, failed, or empty page),
            // fall back to CSS selector extraction.
            let new_jobs = match new_jobs_opt {
                Some(jobs) => {
                    tracing::info!(
                        company   = watch.company_name,
                        extracted = jobs.len(),
                        mode      = "llm",
                        "job extraction complete"
                    );
                    jobs
                }
                None if has_selector => {
                    let selector = watch.job_selector.as_deref().unwrap();
                    match scraper.fetch_jobs_with_selectors(
                        &watch.url, selector,
                        watch.title_selector.as_deref(),
                        watch.location_selector.as_deref(),
                        &watch.company_name,
                    ).await {
                        Ok(jobs) => {
                            tracing::info!(
                                company   = watch.company_name,
                                selector  = selector,
                                extracted = jobs.len(),
                                mode      = "css",
                                "job extraction complete"
                            );
                            jobs
                        }
                        Err(err) => {
                            tracing::warn!(id = %watch.id, "CSS extraction failed: {err}");
                            Vec::new()
                        }
                    }
                }
                None => {
                    tracing::info!(company = watch.company_name, "LLM unavailable and no selector — hash-only");
                    Vec::new()
                }
            };

            let (added, removed) = diff_jobs(&old_jobs, &new_jobs);
            (added, removed, Some(new_jobs))
        } else {
            tracing::info!(company = watch.company_name, "hash-only mode (no Ollama, no selector)");
            (Vec::new(), Vec::new(), None)
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

        // ── Job tracking (analytics layer) ────────────────────────────────────
        match (&watch.company_id, &new_jobs_snapshot) {
            (Some(company_id), Some(current_jobs)) => {
                tracing::info!(
                    company    = watch.company_name,
                    company_id = %company_id,
                    jobs       = current_jobs.len(),
                    "tracking jobs"
                );
                if let Err(e) = track_jobs(pool, *company_id, current_jobs, &removed_jobs, now).await {
                    tracing::warn!(company = watch.company_name, "job tracking failed: {e}");
                } else {
                    tracing::info!(company = watch.company_name, jobs = current_jobs.len(), "jobs upserted");
                }
                let today = now.date_naive();
                if let Err(e) = upsert_daily_snapshot(pool, *company_id, today).await {
                    tracing::warn!(company = watch.company_name, "snapshot update failed: {e}");
                }
            }
            (None, Some(_)) => {
                tracing::info!(company = watch.company_name, "skipping job tracking: no company_id (custom watcher)");
            }
            (Some(_), None) => {
                tracing::info!(company = watch.company_name, "skipping job tracking: no job_selector");
            }
            _ => {}
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
            tracing::warn!(id = %watch.id, "failed to update watcher: {err}");
        }
    }

    Ok(())
}

// ── Job tracking ──────────────────────────────────────────────────────────────

/// Stable hash for a job: SHA-256 of `"<company_id>:<lower_title>:<lower_location>"`.
fn job_hash(company_id: Uuid, title: &str, location: Option<&str>) -> String {
    let input = format!("{}:{}:{}", company_id, title.to_lowercase(), location.unwrap_or("").to_lowercase());
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    h.finalize().iter().map(|b| format!("{:02x}", b)).collect()
}

/// Upsert current jobs and mark removed ones as filled.
async fn track_jobs(
    pool: &PgPool,
    company_id: Uuid,
    current_jobs: &[JobPosting],
    removed_jobs: &[JobPosting],
    now: chrono::DateTime<Utc>,
) -> anyhow::Result<()> {
    tracing::info!(
        company_id = %company_id,
        current    = current_jobs.len(),
        removed    = removed_jobs.len(),
        "track_jobs called"
    );

    // Upsert every job in the current listing
    for job in current_jobs {
        let hash = job_hash(company_id, &job.title, job.location.as_deref());
        // Use LLM-provided department when available; fall back to keyword inference.
        let dept = job.department.as_deref()
            .unwrap_or_else(|| categorize_department(&job.title));
        let (region, is_remote) = categorize_location(job.location.as_deref());

        sqlx::query(
            r#"
            INSERT INTO jobs
                (id, company_id, job_hash, title, raw_location,
                 department_category, location_region, is_remote,
                 url, first_seen_at, last_seen_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10)
            ON CONFLICT (company_id, job_hash) DO UPDATE
                SET last_seen_at = $10,
                    status       = 'active'
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(company_id)
        .bind(&hash)
        .bind(&job.title)
        .bind(job.location.as_deref())
        .bind(dept)
        .bind(region)
        .bind(is_remote)
        .bind(job.url.as_deref())
        .bind(now)
        .execute(pool)
        .await?;
    }

    // Mark removed jobs as filled and compute time-to-fill
    for job in removed_jobs {
        let hash = job_hash(company_id, &job.title, job.location.as_deref());
        sqlx::query(
            r#"
            UPDATE jobs
            SET status            = 'filled',
                filled_at         = $1,
                time_to_fill_days = GREATEST(0, EXTRACT(DAY FROM $1 - first_seen_at)::INTEGER)
            WHERE company_id = $2
              AND job_hash   = $3
              AND status     = 'active'
            "#,
        )
        .bind(now)
        .bind(company_id)
        .bind(&hash)
        .execute(pool)
        .await?;
    }

    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn notification_summary(added: &[JobPosting], removed: &[JobPosting]) -> String {
    match (added.len(), removed.len()) {
        (0, 0) => "Career page updated".to_string(),
        (a, 0) => format!("{} new job{} added", a, if a == 1 { "" } else { "s" }),
        (0, r) => format!("{} job{} removed", r, if r == 1 { "" } else { "s" }),
        (a, r) => format!("{} job{} added, {} removed", a, if a == 1 { "" } else { "s" }, r),
    }
}

fn diff_jobs(old: &[JobPosting], new: &[JobPosting]) -> (Vec<JobPosting>, Vec<JobPosting>) {
    use std::collections::HashSet;
    let old_set: HashSet<_> = old.iter().cloned().collect();
    let new_set: HashSet<_> = new.iter().cloned().collect();
    let added   = new_set.difference(&old_set).cloned().collect();
    let removed = old_set.difference(&new_set).cloned().collect();
    (added, removed)
}

#[cfg(test)]
mod tests {
    use super::{diff_jobs, notification_summary};
    use career_watch::models::JobPosting;

    fn job(title: &str) -> JobPosting {
        JobPosting { title: title.into(), company: "Acme".into(), location: None, department: None, posted_text: None, url: None }
    }

    #[test]
    fn diff_detects_added_and_removed() {
        let old = vec![job("A"), job("B")];
        let new = vec![job("B"), job("C")];
        let (added, removed) = diff_jobs(&old, &new);
        assert_eq!(added, vec![job("C")]);
        assert_eq!(removed, vec![job("A")]);
    }

    #[test]
    fn diff_empty_lists() { let (a, r) = diff_jobs(&[], &[]); assert!(a.is_empty()); assert!(r.is_empty()); }
    #[test]
    fn diff_all_added()   { let (a, r) = diff_jobs(&[], &[job("X"), job("Y")]); assert_eq!(a.len(), 2); assert!(r.is_empty()); }
    #[test]
    fn diff_all_removed() { let (a, r) = diff_jobs(&[job("X"), job("Y")], &[]); assert!(a.is_empty()); assert_eq!(r.len(), 2); }
    #[test]
    fn diff_metadata_only_change() {
        let old = vec![JobPosting { title: "Eng".into(), company: "Acme".into(), location: None, department: None, posted_text: Some("1h ago".into()), url: None }];
        let new = vec![JobPosting { title: "Eng".into(), company: "Acme".into(), location: None, department: None, posted_text: Some("2h ago".into()), url: None }];
        let (a, r) = diff_jobs(&old, &new);
        assert!(a.is_empty()); assert!(r.is_empty());
    }

    #[test]
    fn summary_all_cases() {
        assert_eq!(notification_summary(&[], &[]),                        "Career page updated");
        assert_eq!(notification_summary(&[job("A")], &[]),                "1 new job added");
        assert_eq!(notification_summary(&[job("A"), job("B")], &[]),      "2 new jobs added");
        assert_eq!(notification_summary(&[], &[job("A")]),                "1 job removed");
        assert_eq!(notification_summary(&[job("A")], &[job("B")]),        "1 job added, 1 removed");
    }
}
