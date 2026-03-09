//! Career page prober.
//!
//! Given a company domain, probes a ranked list of common career page paths
//! and returns the first one that validates as a real job listing page.

use anyhow::Result;
use reqwest::{Client, redirect};
use std::time::Duration;
use tracing::{debug, warn};

use super::platform::{Platform, detect_platform, extract_platform_id};
use super::validator::has_job_listings;

/// Ordered list of career page paths to probe, most common first.
const CAREER_PATHS: &[&str] = &[
    "/careers",
    "/jobs",
    "/join-us",
    "/join",
    "/work-with-us",
    "/opportunities",
    "/open-roles",
    "/hiring",
    "/positions",
    "/we-are-hiring",
    "/about/careers",
    "/company/careers",
    "/en/careers",
    "/careers/open-roles",
    "/jobs/open-positions",
];

/// The result of a successful career page probe.
#[derive(Debug, Clone)]
pub struct CareerPageResult {
    /// Final URL after redirects.
    pub url: String,
    pub platform: Platform,
    /// Company slug on the ATS platform, if detected.
    pub platform_id: Option<String>,
    /// Rough estimate of how many jobs are listed.
    pub job_count_estimate: usize,
}

pub struct CareerProber {
    client: Client,
}

impl CareerProber {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(8))
            .user_agent("CareerWatch/1.0 (automated career page discovery)")
            .redirect(redirect::Policy::limited(5))
            .build()?;
        Ok(Self { client })
    }

    /// Probe a bare domain for career pages.
    ///
    /// Tries each path in [`CAREER_PATHS`] in order and returns the first
    /// response that passes validation. Returns `None` if no path yields a
    /// valid career page.
    pub async fn probe_domain(&self, domain: &str) -> Option<CareerPageResult> {
        let base = format!("https://{}", domain.trim_end_matches('/'));

        for path in CAREER_PATHS {
            let url = format!("{}{}", base, path);
            match self.probe_url(&url).await {
                Ok(Some(result)) => {
                    debug!(url, "found career page");
                    return Some(result);
                }
                Ok(None) => continue,
                Err(e) => {
                    warn!(url, error = %e, "probe error");
                    continue;
                }
            }
        }

        None
    }

    /// Probe a specific URL.
    ///
    /// Returns `Ok(Some(_))` when the page validates as a career page,
    /// `Ok(None)` when the page exists but does not look like job listings,
    /// and `Err(_)` on network or decode failures.
    pub async fn probe_url(&self, url: &str) -> Result<Option<CareerPageResult>> {
        let response = match self.client.get(url).send().await {
            Ok(r) => r,
            Err(e) => {
                // Connection errors / timeouts — not worth warning loudly.
                debug!(url, error = %e, "request failed");
                return Ok(None);
            }
        };

        if !response.status().is_success() {
            return Ok(None);
        }

        let final_url = response.url().to_string();
        let html = response.text().await?;

        let platform = detect_platform(&final_url, &html);
        let platform_id = extract_platform_id(&platform, &final_url);
        let (valid, job_count_estimate) = has_job_listings(&html, &platform);

        if !valid {
            return Ok(None);
        }

        Ok(Some(CareerPageResult {
            url: final_url,
            platform,
            platform_id,
            job_count_estimate,
        }))
    }
}
