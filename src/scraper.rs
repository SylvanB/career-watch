//! HTTP scraping and hashing for watched pages.
//!
//! Pages are fetched over HTTP(S) and either hashed with SHA-256 or parsed
//! into structured job postings so that changes can be detected cheaply
//! between worker runs.

use crate::models::JobPosting;
use anyhow::{Context, Result};
use reqwest::Client;
use scraper::{Html, Selector};
use sha2::{Digest, Sha256};

/// Lightweight HTTP client used by the worker to fetch pages.
#[derive(Clone)]
pub struct PageScraper {
    client: Client,
}

impl PageScraper {
    pub fn new() -> Self {
        let client = Client::builder()
            .user_agent("career-watch-bot/0.1")
            .build()
            .expect("failed to build reqwest client");

        Self { client }
    }

    /// Fetch a URL and return the raw HTML body.
    pub async fn fetch_raw_html(&self, url: &str) -> Result<String> {
        self.client
            .get(url)
            .send()
            .await
            .context("failed to fetch page")?
            .error_for_status()
            .context("unexpected HTTP status")?
            .text()
            .await
            .context("failed to read page body")
    }

    /// Fetch a URL and return a SHA-256 hash of the response body.
    ///
    /// The hash is returned as a lowercase hex string. Any network or HTTP
    /// error is propagated as an `anyhow::Error`.
    pub async fn fetch_hash(&self, url: &str) -> Result<String> {
        let body = self
            .client
            .get(url)
            .send()
            .await
            .context("failed to fetch page")?
            .error_for_status()
            .context("unexpected HTTP status when fetching page")?
            .text()
            .await
            .context("failed to read page body")?;

        Ok(hash_body(&body))
    }

    /// Fetch a URL and extract job postings using a CSS selector.
    ///
    /// The `job_selector` is applied per watcher so that different career
    /// sites can be parsed appropriately.
    pub async fn fetch_jobs(
        &self,
        url: &str,
        job_selector: &str,
        company_name: &str,
    ) -> Result<Vec<JobPosting>> {
        let body = self
            .client
            .get(url)
            .send()
            .await
            .context("failed to fetch page")?
            .error_for_status()
            .context("unexpected HTTP status when fetching page")?
            .text()
            .await
            .context("failed to read page body")?;

        extract_jobs_from_html(&body, job_selector, company_name)
    }
}

/// Compute the SHA-256 hash of a response body and return it as hex.
pub fn hash_body(body: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(body.as_bytes());
    let hash = hasher.finalize();
    format!("{:x}", hash)
}

/// Extract job postings from an HTML document using a CSS selector.
///
/// Each element matched by `job_selector` is treated as one job "card".
/// The element's combined text is used as the title, with whitespace
/// normalized. Implementations can be refined per site by choosing
/// tighter selectors.
pub fn extract_jobs_from_html(
    html: &str,
    job_selector: &str,
    company_name: &str,
) -> Result<Vec<JobPosting>> {
    let document = Html::parse_document(html);
    let selector = Selector::parse(job_selector)
        .map_err(|_| anyhow::anyhow!("invalid CSS selector for job extraction"))?;

    let mut jobs = Vec::new();

    for element in document.select(&selector) {
        let mut title = element.text().collect::<String>();
        title = title.split_whitespace().collect::<Vec<_>>().join(" ");
        let title = title.trim().to_string();
        if title.is_empty() {
            continue;
        }

        let posted_text = element
            .value()
            .attr("data-posted")
            .map(|s| s.to_string());

        jobs.push(JobPosting {
            title,
            company: company_name.to_string(),
            location: None,
            posted_text,
            url: None,
        });
    }

    Ok(jobs)
}

#[cfg(test)]
mod tests {
    use super::{extract_jobs_from_html, hash_body};
    use crate::models::JobPosting;

    #[test]
    fn hash_body_is_deterministic() {
        let body = "some sample content";
        let h1 = hash_body(body);
        let h2 = hash_body(body);
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_body_changes_with_input() {
        let a = "first content";
        let b = "second content";
        let ha = hash_body(a);
        let hb = hash_body(b);
        assert_ne!(ha, hb);
    }

    #[test]
    fn extract_jobs_from_simple_cards() {
        let html = r#"
        <div class="job-card" data-posted="14 minutes ago">
            Senior Rust Engineer
        </div>
        <div class="job-card" data-posted="yesterday">
            Quant Researcher
        </div>
        "#;

        let jobs = extract_jobs_from_html(html, ".job-card", "Jane Street").unwrap();
        assert_eq!(jobs.len(), 2);

        let titles: Vec<_> = jobs.iter().map(|j| j.title.as_str()).collect();
        assert_eq!(titles, vec!["Senior Rust Engineer", "Quant Researcher"]);

        assert_eq!(
            jobs[0],
            JobPosting {
                title: "Senior Rust Engineer".to_string(),
                company: "Jane Street".to_string(),
                location: None,
                posted_text: Some("14 minutes ago".to_string()),
                url: None,
            }
        );
    }
}

