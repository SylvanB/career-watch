//! Auto-detection of CSS selectors for job listing elements.
//!
//! ## Strategy
//!
//! 1. **Platform shortcuts** — known ATS platforms have stable, well-known
//!    selectors. Validated against the live HTML before returning.
//!
//! 2. **Generic structural scan** — ranked list of common CSS patterns.
//!    Accepted only when matching ≥ MIN_JOB_COUNT elements AND average text
//!    length is in the title range (4–MAX_TITLE_LEN chars). This rejects
//!    container divs and navigation elements which typically have very long
//!    concatenated text.
//!
//! 3. **Heuristic element scan** — walks `a[href]`, `li`, and `div` elements,
//!    scores those whose class names contain job-related keywords, and accepts
//!    the most common pattern that also passes the text-length gate.

use scraper::{Html, Selector};

use super::platform::Platform;

/// Minimum number of matching elements required to accept a selector.
const MIN_JOB_COUNT: usize = 2;

/// Minimum text length in a matched element to count as a real job title.
const MIN_TITLE_LEN: usize = 4;

/// Maximum average text length across matched elements.
///
/// Real job title elements are short (10–80 chars). If the average is above
/// this threshold the selector is matching a container div or nav element
/// that aggregates many text nodes — discard it.
const MAX_TITLE_LEN: usize = 120;

// ── Public API ────────────────────────────────────────────────────────────────

/// Attempt to detect the CSS selector for individual job listing elements.
pub fn detect_job_selector(html: &str, platform: &Platform) -> Option<String> {
    if let Some(sel) = platform_selector(platform) {
        if selector_ok(html, sel) {
            return Some(sel.to_string());
        }
        // Platform selector didn't match or was a container — fall through.
    }
    detect_generic_selector(html)
}

/// Count how many elements a CSS selector matches in the given HTML.
pub fn selector_hit_count(html: &str, selector: &str) -> usize {
    let document = Html::parse_document(html);
    match Selector::parse(selector) {
        Ok(sel) => document.select(&sel).count(),
        Err(_)  => 0,
    }
}

/// Run a selector and return up to `limit` cleaned title strings.
pub fn selector_sample(html: &str, selector: &str, limit: usize) -> Vec<String> {
    let document = Html::parse_document(html);
    let Ok(sel) = Selector::parse(selector) else { return vec![] };

    document
        .select(&sel)
        .filter_map(|el| {
            let text = el
                .text()
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let text = text.trim().to_string();
            if text.len() >= MIN_TITLE_LEN { Some(text) } else { None }
        })
        .take(limit)
        .collect()
}

// ── Platform shortcuts ────────────────────────────────────────────────────────

fn platform_selector(platform: &Platform) -> Option<&'static str> {
    match platform {
        Platform::Greenhouse     => Some("div.opening a"),
        Platform::Lever          => Some("div.posting h5"),
        Platform::SmartRecruiters => Some(".job-list-item h4"),
        Platform::Ashby | Platform::Workday => None,
        _ => None,
    }
}

/// Return true if the selector matches ≥ MIN_JOB_COUNT elements with an
/// average text length in the title range.
fn selector_ok(html: &str, selector: &str) -> bool {
    let document = Html::parse_document(html);
    let Ok(sel) = Selector::parse(selector) else { return false };
    let els: Vec<_> = document.select(&sel).collect();
    if els.len() < MIN_JOB_COUNT { return false; }

    let avg = avg_text_len(&els);
    avg >= MIN_TITLE_LEN && avg <= MAX_TITLE_LEN
}

// ── Generic scan ──────────────────────────────────────────────────────────────

fn detect_generic_selector(html: &str) -> Option<String> {
    let document = Html::parse_document(html);

    let candidates: &[&str] = &[
        "[data-job-id]",
        "[data-jobid]",
        "[data-job-slug]",
        "[data-job-title]",
        "[data-position-id]",
        ".job-card",
        ".job-listing",
        ".job-item",
        ".job-row",
        ".job-post",
        ".position-card",
        ".position-item",
        ".opening-item",
        ".career-item",
        ".role-card",
        ".role-item",
        ".jobs li",
        ".job-list li",
        ".job-listings li",
        ".careers-list li",
        ".positions-list li",
        ".openings-list li",
        ".jobs-table tr",
        ".positions-table tr",
    ];

    for &pattern in candidates {
        if let Some(sel) = try_selector(&document, pattern) {
            return Some(sel);
        }
    }

    heuristic_scan(&document)
}

fn try_selector(document: &Html, pattern: &str) -> Option<String> {
    let sel = Selector::parse(pattern).ok()?;
    let elements: Vec<_> = document.select(&sel).collect();

    if elements.len() < MIN_JOB_COUNT {
        return None;
    }

    let avg = avg_text_len(&elements);
    if avg < MIN_TITLE_LEN || avg > MAX_TITLE_LEN {
        return None; // Container element or empty — skip
    }

    Some(pattern.to_string())
}

/// Check whether `pattern` matches elements in the document that look like
/// job titles (correct count and average text length).
/// The Selector lifetime is entirely contained within this function.
fn pattern_ok(document: &Html, pattern: &str) -> bool {
    match Selector::parse(pattern) {
        Ok(sel) => {
            let els: Vec<_> = document.select(&sel).collect();
            let avg = avg_text_len(&els);
            avg >= MIN_TITLE_LEN && avg <= MAX_TITLE_LEN
        }
        Err(_) => false,
    }
}

fn avg_text_len(elements: &[scraper::ElementRef]) -> usize {
    if elements.is_empty() { return 0; }
    let total: usize = elements
        .iter()
        .map(|el| el.text().collect::<String>().trim().len())
        .sum();
    total / elements.len()
}

// ── Heuristic scan ────────────────────────────────────────────────────────────

const JOB_CLASS_KEYWORDS: &[&str] = &[
    "job", "position", "opening", "role", "career",
    "vacancy", "opportunity", "listing", "posting",
];

fn heuristic_scan(document: &Html) -> Option<String> {
    use std::collections::HashMap;

    // Include all element types but rely on the text-length gate (applied per
    // element below) to reject container divs that aggregate many text nodes.
    let Ok(sel) = Selector::parse(
        "a[href][class], li[class], h3[class], h4[class], h5[class], div[class], span[class]",
    ) else {
        return None;
    };

    let mut counts: HashMap<String, usize> = HashMap::new();

    for el in document.select(&sel) {
        // Text length gate — ignore elements that are too short or too long.
        let text_len = el.text().collect::<String>().trim().len();
        if text_len < MIN_TITLE_LEN || text_len > MAX_TITLE_LEN {
            continue;
        }

        let classes = el.value().attr("class").unwrap_or("").split_whitespace();
        for class in classes {
            let lower = class.to_lowercase();
            if JOB_CLASS_KEYWORDS.iter().any(|kw| lower.contains(kw)) {
                let tag = el.value().name();
                let pattern = format!("{}.{}", tag, class);
                *counts.entry(pattern).or_insert(0) += 1;
            }
        }
    }

    let mut sorted: Vec<(String, usize)> = counts
        .into_iter()
        .filter(|(_, c)| *c >= MIN_JOB_COUNT)
        .collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));

    for (pattern, _) in &sorted {
        if pattern_ok(document, pattern) {
            return Some(pattern.clone());
        }
    }

    None
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn greenhouse_html() -> &'static str {
        r#"<div class="opening"><a href="/jobs/1">Senior Engineer</a><span class="location">SF</span></div>
           <div class="opening"><a href="/jobs/2">Product Manager</a><span class="location">NY</span></div>
           <div class="opening"><a href="/jobs/3">Data Scientist</a><span class="location">Remote</span></div>"#
    }

    fn lever_html() -> &'static str {
        r#"<div class="posting"><h5>Backend Engineer</h5></div>
           <div class="posting"><h5>Frontend Engineer</h5></div>
           <div class="posting"><h5>Product Designer</h5></div>"#
    }

    #[test]
    fn greenhouse_selector_validated_against_html() {
        let sel = detect_job_selector(greenhouse_html(), &Platform::Greenhouse);
        assert_eq!(sel.as_deref(), Some("div.opening a"));
    }

    #[test]
    fn greenhouse_falls_back_when_html_empty() {
        assert!(detect_job_selector("", &Platform::Greenhouse).is_none());
    }

    #[test]
    fn lever_selector_validated_against_html() {
        let sel = detect_job_selector(lever_html(), &Platform::Lever);
        assert_eq!(sel.as_deref(), Some("div.posting h5"));
    }

    #[test]
    fn lever_falls_back_to_generic_when_html_empty() {
        assert!(detect_job_selector("", &Platform::Lever).is_none());
    }

    #[test]
    fn ashby_has_no_platform_shortcut() {
        assert!(detect_job_selector("", &Platform::Ashby).is_none());
        // With realistic job titles (> MIN_TITLE_LEN), generic detection finds the selector.
        let html = r#"
            <div class="job-card">Senior Software Engineer</div>
            <div class="job-card">Product Manager</div>
            <div class="job-card">Data Scientist</div>
        "#;
        assert!(detect_job_selector(html, &Platform::Ashby).is_some());
    }

    #[test]
    fn workday_returns_none() {
        assert!(detect_job_selector("", &Platform::Workday).is_none());
    }

    #[test]
    fn rejects_long_text_container_elements() {
        // A nav element whose text is 200+ chars should be rejected
        let long_text = "x".repeat(200);
        let html = format!(
            r#"<div class="job-nav">{t}</div><div class="job-nav">{t}</div><div class="job-nav">{t}</div>"#,
            t = long_text
        );
        assert!(detect_job_selector(&html, &Platform::Unknown).is_none(),
            "container with long text should not be accepted as a job selector");
    }

    #[test]
    fn accepts_normal_length_job_cards() {
        let html = r#"
            <div class="job-card">Senior Software Engineer</div>
            <div class="job-card">Product Manager</div>
            <div class="job-card">Data Scientist</div>
        "#;
        let sel = detect_job_selector(html, &Platform::Unknown);
        assert_eq!(sel.as_deref(), Some(".job-card"));
    }

    #[test]
    fn selector_hit_count_counts_matches() {
        assert_eq!(selector_hit_count(greenhouse_html(), "div.opening a"), 3);
        assert_eq!(selector_hit_count(greenhouse_html(), ".nonexistent"), 0);
    }

    #[test]
    fn selector_sample_returns_titles() {
        let sample = selector_sample(greenhouse_html(), "div.opening a", 5);
        assert_eq!(sample.len(), 3);
        assert!(sample.contains(&"Senior Engineer".to_string()));
    }

    #[test]
    fn selector_sample_respects_limit() {
        let sample = selector_sample(greenhouse_html(), "div.opening a", 2);
        assert_eq!(sample.len(), 2);
    }
}
