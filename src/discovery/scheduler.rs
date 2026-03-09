//! Discovery queue and crawl scheduler.
//!
//! The scheduler manages two operations:
//!
//! 1. **Enqueue** — inserts new `CompanyLead` rows into the `companies` table,
//!    deduplicating by domain using `ON CONFLICT DO NOTHING`.
//! 2. **Probe** — fetches pending (or stale) companies from the queue, runs
//!    the prober against each domain, and writes results back to the database.
//!
//! Concurrency is bounded by a semaphore to avoid overwhelming the network or
//! the database connection pool.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chrono::Utc;
use futures::StreamExt;
use sqlx::PgPool;
use tokio::sync::Semaphore;
use tracing::{info, warn};
use uuid::Uuid;

use super::prober::CareerProber;
use super::sources::CompanyLead;

/// How many companies to probe concurrently.
const PROBE_CONCURRENCY: usize = 8;

/// Re-probe active companies whose career page was last checked more than this
/// many hours ago.
const REPROBE_AFTER_HOURS: i64 = 24;

/// A row fetched from the `companies` table for probing.
#[derive(Debug, sqlx::FromRow)]
pub struct CompanyRow {
    pub id: Uuid,
    pub name: String,
    pub domain: String,
    pub source: String,
    pub status: String,
}

pub struct DiscoveryScheduler {
    db: PgPool,
    prober: Arc<CareerProber>,
}

impl DiscoveryScheduler {
    pub fn new(db: PgPool, prober: CareerProber) -> Self {
        Self {
            db,
            prober: Arc::new(prober),
        }
    }

    // ── Enqueue ──────────────────────────────────────────────────────────────

    /// Insert company leads into the discovery queue.
    ///
    /// Skips any domain already present in the `companies` table.
    /// Returns the number of newly inserted rows.
    pub async fn enqueue_companies(&self, leads: Vec<CompanyLead>) -> Result<usize> {
        let mut inserted = 0usize;

        for lead in leads {
            let rows_affected = sqlx::query(
                r#"
                INSERT INTO companies (id, name, domain, source)
                VALUES ($1, $2, $3, $4)
                ON CONFLICT (domain) DO NOTHING
                "#,
            )
            .bind(Uuid::new_v4())
            .bind(&lead.name)
            .bind(&lead.domain)
            .bind(&lead.source)
            .execute(&self.db)
            .await?
            .rows_affected();

            if rows_affected > 0 {
                inserted += 1;
                info!(domain = %lead.domain, "enqueued company");
            }
        }

        Ok(inserted)
    }

    // ── Dequeue ───────────────────────────────────────────────────────────────

    /// Fetch companies pending their first probe.
    pub async fn pending_companies(&self, limit: i64) -> Result<Vec<CompanyRow>> {
        Ok(sqlx::query_as::<_, CompanyRow>(
            r#"
            SELECT id, name, domain, source, status
            FROM companies
            WHERE status = 'pending'
            ORDER BY created_at ASC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.db)
        .await?)
    }

    /// Fetch active companies whose career page has not been re-checked recently.
    pub async fn stale_companies(&self, limit: i64) -> Result<Vec<CompanyRow>> {
        let cutoff = Utc::now() - chrono::Duration::hours(REPROBE_AFTER_HOURS);

        Ok(sqlx::query_as::<_, CompanyRow>(
            r#"
            SELECT id, name, domain, source, status
            FROM companies
            WHERE status = 'active'
              AND (last_probed_at IS NULL OR last_probed_at < $1)
            ORDER BY last_probed_at ASC NULLS FIRST
            LIMIT $2
            "#,
        )
        .bind(cutoff)
        .bind(limit)
        .fetch_all(&self.db)
        .await?)
    }

    // ── Probe ────────────────────────────────────────────────────────────────

    /// Probe a batch of companies concurrently, bounded by [`PROBE_CONCURRENCY`].
    pub async fn probe_all(&self, companies: Vec<CompanyRow>) -> Result<()> {
        let sem = Arc::new(Semaphore::new(PROBE_CONCURRENCY));

        futures::stream::iter(companies)
            .for_each_concurrent(PROBE_CONCURRENCY, |company| {
                let sem = sem.clone();
                let prober = self.prober.clone();
                let db = self.db.clone();

                async move {
                    let _permit = sem.acquire().await.expect("semaphore closed");
                    if let Err(e) = probe_one(&db, &prober, &company).await {
                        warn!(domain = %company.domain, error = %e, "probe failed");
                    }
                }
            })
            .await;

        Ok(())
    }
}

/// Probe a single company and write the result to the database.
async fn probe_one(db: &PgPool, prober: &CareerProber, company: &CompanyRow) -> Result<()> {
    info!(domain = %company.domain, "probing");

    match prober.probe_domain(&company.domain).await {
        Some(result) => {
            info!(
                domain     = %company.domain,
                url        = %result.url,
                platform   = result.platform.as_str(),
                score      = result.score,
                source     = result.source,
                jobs       = result.job_count_estimate,
                "career page found"
            );

            sqlx::query(
                r#"
                UPDATE companies
                SET status            = 'active',
                    career_url        = $1,
                    platform          = $2,
                    platform_id       = $3,
                    probe_confidence  = $4,
                    probe_source      = $5,
                    job_selector      = $6,
                    title_selector    = $7,
                    location_selector = $8,
                    last_probed_at    = $9,
                    probe_error       = NULL
                WHERE id = $10
                "#,
            )
            .bind(&result.url)
            .bind(result.platform.as_str())
            .bind(result.platform_id.as_deref())
            .bind(result.score as i16)
            .bind(result.source)
            .bind(result.job_selector.as_deref())
            .bind(result.title_selector.as_deref())
            .bind(result.location_selector.as_deref())
            .bind(Utc::now())
            .bind(company.id)
            .execute(db)
            .await?;
        }
        None => {
            info!(domain = %company.domain, "no career page found");

            sqlx::query(
                r#"
                UPDATE companies
                SET status        = 'failed',
                    probe_error   = 'no career page found',
                    last_probed_at = $1
                WHERE id = $2
                "#,
            )
            .bind(Utc::now())
            .bind(company.id)
            .execute(db)
            .await?;
        }
    }

    Ok(())
}

/// Build a default reqwest [`Client`] suitable for seed-source fetching.
pub fn build_http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent("CareerWatch/1.0 (automated career page discovery)")
        .build()?)
}
