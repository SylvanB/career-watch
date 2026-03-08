//! Binary entrypoint for the Career Watch worker.
//!
//! This binary is intended to be run periodically (e.g. by cron). It scans
//! all active watchers, fetches their pages, and sends notification emails
//! when a change is detected. When possible it computes a structured diff
//! of added/removed job postings.

use career_watch::config::AppConfig;
use career_watch::db::create_pool;
use career_watch::email::EmailClient;
use career_watch::models::{JobPosting, WatchRow};
use career_watch::scraper::PageScraper;
use chrono::Utc;
use sqlx::types::Json;
use sqlx::PgPool;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

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

async fn run_once(
    pool: &PgPool,
    email: &EmailClient,
    scraper: &PageScraper,
) -> anyhow::Result<()> {
    let watches: Vec<WatchRow> = sqlx::query_as::<_, WatchRow>(
        r#"
        SELECT id, company_name, url, email, last_hash, last_checked_at, created_at, active, last_jobs_json, job_selector
        FROM watchers
        WHERE active = true
        "#,
    )
    .fetch_all(pool)
    .await?;

    for watch in watches {
        let now = Utc::now();

        let (new_hash, added_jobs, removed_jobs, new_jobs_snapshot) =
            match &watch.job_selector {
                Some(selector) if !selector.trim().is_empty() => {
                    // Structured job-based mode.
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
                                "failed to fetch jobs for watcher {} ({}): {err:?}",
                                watch.id,
                                watch.url
                            );
                            continue;
                        }
                    };

                    let (added, removed) = diff_jobs(&old_jobs, &new_jobs);

                    // Still compute a content hash for coarse change tracking.
                    let hash = match scraper.fetch_hash(&watch.url).await {
                        Ok(h) => h,
                        Err(err) => {
                            tracing::warn!(
                                "failed to fetch hash for watcher {} ({}): {err:?}",
                                watch.id,
                                watch.url
                            );
                            continue;
                        }
                    };

                    (hash, added, removed, Some(new_jobs))
                }
                _ => {
                    // Legacy hash-only mode.
                    let new_hash = match scraper.fetch_hash(&watch.url).await {
                        Ok(h) => h,
                        Err(err) => {
                            tracing::warn!(
                                "failed to fetch page for watcher {} ({}): {err:?}",
                                watch.id,
                                watch.url
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
        } || !added_jobs.is_empty() || !removed_jobs.is_empty();

        if should_notify {
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
                tracing::warn!(
                    "failed to send email for watcher {}: {err:?}",
                    watch.id
                );
            }
        }

        let jobs_json: Option<Json<Vec<JobPosting>>> =
            new_jobs_snapshot.map(|jobs| Json(jobs));

        if let Err(err) = sqlx::query(
            r#"
            UPDATE watchers
            SET last_hash = $1, last_checked_at = $2, last_jobs_json = $3
            WHERE id = $4
            "#,
        )
        .bind(&new_hash)
        .bind(now)
        .bind(jobs_json)
        .bind(watch.id)
        .execute(pool)
        .await
        {
            tracing::warn!(
                "failed to update watcher {} after scrape: {err:?}",
                watch.id
            );
        }
    }

    Ok(())
}

fn diff_jobs(old: &[JobPosting], new: &[JobPosting]) -> (Vec<JobPosting>, Vec<JobPosting>) {
    use std::collections::HashSet;

    let old_set: HashSet<_> = old.iter().cloned().collect();
    let new_set: HashSet<_> = new.iter().cloned().collect();

    let added = new_set
        .difference(&old_set)
        .cloned()
        .collect::<Vec<_>>();
    let removed = old_set
        .difference(&new_set)
        .cloned()
        .collect::<Vec<_>>();

    (added, removed)
}

#[cfg(test)]
mod tests {
    use super::diff_jobs;
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

    #[test]
    fn diff_jobs_detects_added_and_removed() {
        let old = vec![job("A"), job("B")];
        let new = vec![job("B"), job("C")];

        let (added, removed) = diff_jobs(&old, &new);

        assert_eq!(added, vec![job("C")]);
        assert_eq!(removed, vec![job("A")]);
    }
}

