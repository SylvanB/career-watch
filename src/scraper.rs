//! HTTP scraping and hashing for watched pages.
//!
//! Pages are fetched over HTTP(S) and either hashed with SHA-256 or parsed
//! into structured job postings so that changes can be detected cheaply
//! between worker runs.

use crate::models::JobPosting;
use anyhow::{Context, Result};
use reqwest::Client;
use scraper::{ElementRef, Html, Selector};
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
/// For each matched element the extractor:
/// 1. Looks for a dedicated location child element (class containing
///    "location", "city", "region", or "place").
/// 2. If found, reads it separately and uses the remaining text as the title.
/// 3. If not found and the full text looks like "TitleCity, State" (no
///    separator), splits at the camelCase boundary.
/// 4. Falls back to using the full text as the title with no location.
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
        // Try to find a dedicated location element inside the card.
        let location_from_child = find_location_child(&element);

        // Collect the full text of the matched element.
        let full_text: String = element.text().collect::<Vec<_>>().join(" ");
        let full_text = full_text.split_whitespace().collect::<Vec<_>>().join(" ");
        let full_text = full_text.trim().to_string();

        if full_text.is_empty() {
            continue;
        }

        // Derive the title and location.
        let (title, location) = if let Some(ref loc) = location_from_child {
            // Strip the location text from the full string to get a clean title.
            let title = full_text
                .replace(loc.as_str(), "")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .to_string();
            (title, location_from_child)
        } else {
            // No child location element — try splitting at a case boundary.
            split_title_from_location(&full_text)
        };

        if title.is_empty() {
            continue;
        }

        let posted_text = element
            .value()
            .attr("data-posted")
            .map(|s| s.to_string());

        let url = element
            .value()
            .attr("href")
            .or_else(|| {
                // Look for an <a href> child if the matched element isn't itself a link
                Selector::parse("a[href]").ok().and_then(|s| {
                    element.select(&s).next()?.value().attr("href")
                })
            })
            .map(|s| s.to_string());

        jobs.push(JobPosting {
            title,
            company: company_name.to_string(),
            location,
            posted_text,
            url,
        });
    }

    Ok(jobs)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Location-related class name keywords.
const LOCATION_CLASS_KEYWORDS: &[&str] = &[
    "location", "city", "region", "place", "office", "geo",
];

/// Find the first child element whose class name suggests it holds location data.
/// Returns the trimmed text of that element, or None.
fn find_location_child(el: &ElementRef) -> Option<String> {
    let Ok(any) = Selector::parse("*") else { return None };
    for child in el.select(&any) {
        let class = child.value().attr("class").unwrap_or("");
        let lower = class.to_lowercase();
        if LOCATION_CLASS_KEYWORDS.iter().any(|kw| lower.contains(kw)) {
            let text = child.text().collect::<String>();
            let text = text.trim().to_string();
            if !text.is_empty() && text.len() < 100 {
                return Some(text);
            }
        }
    }
    None
}

/// Split a concatenated "TitleCity, State" string into (title, Some(location)).
///
/// Many ATS pages render the title and location as adjacent text nodes with no
/// separator. When the title element is a container, the scraper sees both
/// concatenated. We detect this by finding a lowercase→uppercase boundary
/// (`rS` in "EngineerSan Francisco") where the remainder looks like a location.
///
/// Returns `(original, None)` when no split point is found.
pub fn split_title_from_location(text: &str) -> (String, Option<String>) {
    let chars: Vec<char> = text.chars().collect();

    for i in 1..chars.len() {
        // Look for [lowercase][Uppercase] with no space between them.
        if chars[i - 1].is_lowercase() && chars[i].is_uppercase() {
            let candidate = &text[chars[..i].iter().map(|c| c.len_utf8()).sum::<usize>()..];
            if looks_like_location(candidate) {
                let boundary = chars[..i].iter().map(|c| c.len_utf8()).sum::<usize>();
                let title    = text[..boundary].trim().to_string();
                let location = candidate.trim().to_string();
                if !title.is_empty() && !location.is_empty() {
                    return (title, Some(location));
                }
            }
        }
    }

    (text.trim().to_string(), None)
}

/// Return true if the string looks like a location (City, State/Country or "Remote").
fn looks_like_location(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s.len() > 80 { return false; }

    // Must start with uppercase
    if !s.starts_with(|c: char| c.is_uppercase()) { return false; }

    // "City, State" or "City, Country" — most common format
    if s.contains(", ") { return true; }

    // Single-word remote/global patterns
    const SINGLE_WORD: &[&str] = &["Remote", "Worldwide", "Anywhere", "Global", "Virtual"];
    SINGLE_WORD.iter().any(|&w| s == w || s.starts_with(&format!("{} ", w)))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── hash ──────────────────────────────────────────────────────────────────

    #[test]
    fn hash_body_is_deterministic() {
        let h1 = hash_body("some sample content");
        let h2 = hash_body("some sample content");
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_body_changes_with_input() {
        assert_ne!(hash_body("first content"), hash_body("second content"));
    }

    // ── basic extraction ──────────────────────────────────────────────────────

    #[test]
    fn extract_jobs_from_simple_cards() {
        let html = r#"
        <div class="job-card" data-posted="14 minutes ago">Senior Rust Engineer</div>
        <div class="job-card" data-posted="yesterday">Quant Researcher</div>
        "#;

        let jobs = extract_jobs_from_html(html, ".job-card", "Jane Street").unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].title, "Senior Rust Engineer");
        assert_eq!(jobs[0].posted_text.as_deref(), Some("14 minutes ago"));
        assert_eq!(jobs[0].location, None); // clean text, no location to extract
    }

    // ── location child extraction ─────────────────────────────────────────────

    #[test]
    fn extracts_location_from_child_element() {
        let html = r#"
        <div class="opening">
          <a href="/jobs/1">Senior Engineer</a>
          <span class="location">San Francisco, California</span>
        </div>
        <div class="opening">
          <a href="/jobs/2">Product Manager</a>
          <span class="location">New York, New York</span>
        </div>
        "#;
        let jobs = extract_jobs_from_html(html, ".opening", "Acme").unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].title, "Senior Engineer");
        assert_eq!(jobs[0].location.as_deref(), Some("San Francisco, California"));
        assert_eq!(jobs[1].location.as_deref(), Some("New York, New York"));
    }

    #[test]
    fn extracts_url_from_child_anchor() {
        let html = r#"
        <div class="job-card"><a href="/jobs/eng">Backend Engineer</a><span class="city">Remote</span></div>
        <div class="job-card"><a href="/jobs/pm">Product Manager</a><span class="city">NYC</span></div>
        "#;
        let jobs = extract_jobs_from_html(html, ".job-card", "Acme").unwrap();
        assert_eq!(jobs[0].url.as_deref(), Some("/jobs/eng"));
    }

    // ── title/location split ──────────────────────────────────────────────────

    #[test]
    fn splits_concatenated_title_and_location() {
        let (title, loc) = split_title_from_location("AI Applications EngineerSan Francisco, California");
        assert_eq!(title, "AI Applications Engineer");
        assert_eq!(loc.as_deref(), Some("San Francisco, California"));
    }

    #[test]
    fn splits_remote_location() {
        let (title, loc) = split_title_from_location("Senior Product ManagerRemote");
        assert_eq!(title, "Senior Product Manager");
        assert_eq!(loc.as_deref(), Some("Remote"));
    }

    #[test]
    fn no_split_when_no_location() {
        let (title, loc) = split_title_from_location("Senior Software Engineer");
        assert_eq!(title, "Senior Software Engineer");
        assert!(loc.is_none());
    }

    #[test]
    fn no_split_for_camelcase_titles() {
        // "VP" starts with uppercase but shouldn't trigger a split
        let (title, loc) = split_title_from_location("VP of Engineering");
        assert_eq!(title, "VP of Engineering");
        assert!(loc.is_none(), "VP→o is not a location");
    }

    #[test]
    fn split_used_in_extraction_when_no_child_location() {
        let html = r#"
        <li class="opening">AI Applications EngineerSan Francisco, California</li>
        <li class="opening">Product ManagerNew York, New York</li>
        "#;
        let jobs = extract_jobs_from_html(html, ".opening", "Notion").unwrap();
        assert_eq!(jobs[0].title, "AI Applications Engineer");
        assert_eq!(jobs[0].location.as_deref(), Some("San Francisco, California"));
        assert_eq!(jobs[1].title, "Product Manager");
        assert_eq!(jobs[1].location.as_deref(), Some("New York, New York"));
    }

    // ── looks_like_location ───────────────────────────────────────────────────

    #[test]
    fn location_detection() {
        assert!(looks_like_location("San Francisco, California"));
        assert!(looks_like_location("London, United Kingdom"));
        assert!(looks_like_location("Remote"));
        assert!(looks_like_location("Worldwide"));
        assert!(!looks_like_location("of Engineering"));  // starts lowercase after 'o'
        assert!(!looks_like_location(""));
        assert!(!looks_like_location(&"x".repeat(100))); // too long
    }
}
