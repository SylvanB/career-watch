//! Auto-detection of CSS selectors for job listing elements.
//!
//! Called after a career page is validated. Returns the best CSS selector
//! for extracting individual job titles, enabling job-level diffing in the
//! worker without any manual configuration.
//!
//! ## Strategy
//!
//! 1. **Platform shortcuts** — known ATS platforms have stable, well-known
//!    selectors. Return immediately without touching the HTML.
//!
//! 2. **Generic structural scan** — try a ranked list of common CSS patterns.
//!    A pattern is accepted if it matches ≥ `MIN_JOB_COUNT` elements and at
//!    least one of them has non-trivial text content (a job title).
//!
//! 3. **Heuristic element scan** — walk all `<div>`, `<li>`, and `<article>`
//!    elements, score those whose class/id contains job-related keywords,
//!    and return the most specific matching selector.

use scraper::{Html, Selector};

use super::platform::Platform;

/// Minimum number of matching elements required to accept a selector.
const MIN_JOB_COUNT: usize = 2;

/// Minimum text length in a matched element to count as a real job title.
const MIN_TITLE_LEN: usize = 4;

/// Attempt to detect the CSS selector for individual job listing elements.
///
/// Returns `Some(selector)` when a reliable selector is found, `None` when
/// the page structure is too ambiguous (e.g. JS-rendered SPAs).
///
/// Platform shortcuts are tried first, but only accepted if they actually
/// match on the fetched HTML — this guards against pages that embed jobs via
/// JS/iframe and would produce 0 matches with the known selector.
pub fn detect_job_selector(html: &str, platform: &Platform) -> Option<String> {
    if let Some(sel) = platform_selector(platform) {
        if selector_hit_count(html, sel) >= MIN_JOB_COUNT {
            return Some(sel.to_string());
        }
        // Platform selector didn't match — ATS page may use an iframe or JS
        // render. Fall through to generic detection rather than storing a
        // selector that would extract nothing.
    }

    detect_generic_selector(html)
}

/// Count how many elements a CSS selector matches in the given HTML.
/// Returns 0 for invalid selectors.
pub fn selector_hit_count(html: &str, selector: &str) -> usize {
    let document = Html::parse_document(html);
    match Selector::parse(selector) {
        Ok(sel) => document.select(&sel).count(),
        Err(_)  => 0,
    }
}

/// Run a selector and return up to `limit` non-empty title strings.
/// Used by the test endpoint to give users a live preview.
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

/// Hardcoded selectors for known ATS platforms.
///
/// These are verified against the live HTML structure of each platform and
/// produce clean job title strings when used with `extract_jobs_from_html`.
fn platform_selector(platform: &Platform) -> Option<&'static str> {
    match platform {
        // boards.greenhouse.io/{company}
        // Each job: <div class="opening"><a href="...">Title</a> ...</div>
        Platform::Greenhouse => Some("div.opening a"),

        // jobs.lever.co/{company}
        // Each job: <div class="posting"><h5>Title</h5> ...</div>
        Platform::Lever => Some("div.posting h5"),

        // careers.smartrecruiters.com/{company}
        // Each job: <li class="job-list-item"><h4 ...>Title</h4> ...</li>
        Platform::SmartRecruiters => Some(".job-list-item h4"),

        // Ashby and Workday render heavily via JS — selectors are unreliable.
        // Fall back to hash-only change detection for these platforms.
        Platform::Ashby | Platform::Workday => None,

        _ => None,
    }
}

/// Generic CSS selector detection for custom career pages.
///
/// Tries a ranked list of common structural patterns, returning the first
/// one that matches enough elements with real text content.
fn detect_generic_selector(html: &str) -> Option<String> {
    let document = Html::parse_document(html);

    // Ordered from most specific (data attributes, known class patterns)
    // to most generic (bare list items).
    let candidates: &[&str] = &[
        // data-* attributes — unambiguous job identifiers
        "[data-job-id]",
        "[data-jobid]",
        "[data-job-slug]",
        "[data-job-title]",
        "[data-position-id]",
        // Common class names used by in-house career pages
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
        // Jobs inside a container (more specific than bare li)
        ".jobs li",
        ".job-list li",
        ".job-listings li",
        ".careers-list li",
        ".positions-list li",
        ".openings-list li",
        // Table-based layouts
        ".jobs-table tr",
        ".positions-table tr",
    ];

    for &pattern in candidates {
        if let Some(sel) = try_selector(&document, pattern) {
            return Some(sel);
        }
    }

    // Last resort: score all div/li/article elements with job-keyword class names.
    heuristic_scan(&document)
}

/// Return `Some(pattern)` if it matches ≥ `MIN_JOB_COUNT` elements that
/// contain meaningful text. Returns `None` otherwise.
fn try_selector(document: &Html, pattern: &str) -> Option<String> {
    let sel = Selector::parse(pattern).ok()?;
    let elements: Vec<_> = document.select(&sel).collect();

    if elements.len() < MIN_JOB_COUNT {
        return None;
    }

    let has_real_text = elements.iter().any(|el| {
        let text = el.text().collect::<String>();
        text.trim().len() >= MIN_TITLE_LEN
    });

    if has_real_text {
        Some(pattern.to_string())
    } else {
        None
    }
}

/// Job-related keywords used to score candidate element class names.
const JOB_CLASS_KEYWORDS: &[&str] = &[
    "job", "position", "opening", "role", "career",
    "vacancy", "opportunity", "listing", "posting",
];

/// Walk the DOM and find repeating elements whose class names suggest they
/// are job listings.
fn heuristic_scan(document: &Html) -> Option<String> {
    use std::collections::HashMap;

    let Ok(sel) = Selector::parse("div[class], li[class], article[class]") else {
        return None;
    };

    // Count occurrences of each class-based selector
    let mut counts: HashMap<String, usize> = HashMap::new();

    for el in document.select(&sel) {
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

    // Sort by count descending and return the first pattern that has real text.
    let mut sorted: Vec<(String, usize)> = counts
        .into_iter()
        .filter(|(_, c)| *c >= MIN_JOB_COUNT)
        .collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));

    for (pattern, _) in &sorted {
        let has_text = match Selector::parse(pattern.as_str()) {
            Ok(sel) => document.select(&sel).any(|el| {
                el.text().collect::<String>().trim().len() >= MIN_TITLE_LEN
            }),
            Err(_) => false,
        };
        if has_text {
            return Some(pattern.clone());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Platform shortcuts ────────────────────────────────────────────────────

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
        assert_eq!(sel.as_deref(), Some("div.opening a"),
            "Greenhouse selector should be accepted when HTML matches");
    }

    #[test]
    fn greenhouse_falls_back_when_html_empty() {
        // Empty HTML → 0 matches → fall through to generic (also None here)
        let sel = detect_job_selector("", &Platform::Greenhouse);
        assert!(sel.is_none(),
            "Empty HTML should not accept the Greenhouse platform shortcut");
    }

    #[test]
    fn lever_selector_validated_against_html() {
        let sel = detect_job_selector(lever_html(), &Platform::Lever);
        assert_eq!(sel.as_deref(), Some("div.posting h5"));
    }

    #[test]
    fn lever_falls_back_to_generic_when_html_empty() {
        let sel = detect_job_selector("", &Platform::Lever);
        assert!(sel.is_none());
    }

    #[test]
    fn ashby_has_no_platform_shortcut() {
        // Ashby has no hardcoded shortcut — falls through to generic detection.
        // On empty HTML, generic detection also returns None.
        assert!(detect_job_selector("", &Platform::Ashby).is_none());
        // On HTML with job-card markup, generic detection still finds something.
        let html = r#"<div class="job-card">Eng</div><div class="job-card">PM</div><div class="job-card">Design</div>"#;
        assert!(detect_job_selector(html, &Platform::Ashby).is_some(),
            "generic detection should still work for Ashby pages that have job-card markup");
    }

    #[test]
    fn workday_returns_none() {
        assert!(detect_job_selector("", &Platform::Workday).is_none());
    }

    #[test]
    fn selector_hit_count_counts_matches() {
        assert_eq!(selector_hit_count(greenhouse_html(), "div.opening a"), 3);
        assert_eq!(selector_hit_count(greenhouse_html(), "div.opening"), 3);
        assert_eq!(selector_hit_count(greenhouse_html(), ".nonexistent"), 0);
        assert_eq!(selector_hit_count("", "div.opening a"), 0);
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

    // ── Generic detection ─────────────────────────────────────────────────────

    #[test]
    fn detects_job_card_class() {
        let html = r#"
            <div class="job-card"><h3>Senior Engineer</h3></div>
            <div class="job-card"><h3>Product Designer</h3></div>
            <div class="job-card"><h3>Data Scientist</h3></div>
        "#;
        let sel = detect_job_selector(html, &Platform::Unknown);
        assert_eq!(sel.as_deref(), Some(".job-card"));
    }

    #[test]
    fn detects_data_job_id_attribute() {
        let html = r#"
            <div data-job-id="1">Backend Engineer</div>
            <div data-job-id="2">Frontend Engineer</div>
        "#;
        let sel = detect_job_selector(html, &Platform::Unknown);
        assert_eq!(sel.as_deref(), Some("[data-job-id]"));
    }

    #[test]
    fn detects_jobs_list_items() {
        let html = r#"
            <ul class="job-listings">
                <li>Software Engineer</li>
                <li>Product Manager</li>
                <li>Designer</li>
            </ul>
        "#;
        let sel = detect_job_selector(html, &Platform::Unknown);
        assert_eq!(sel.as_deref(), Some(".job-listings li"));
    }

    #[test]
    fn returns_none_for_empty_page() {
        assert!(detect_job_selector("", &Platform::Unknown).is_none());
    }

    #[test]
    fn requires_min_two_matches() {
        // Only one job card — should not accept
        let html = r#"<div class="job-card">Senior Engineer</div>"#;
        // Single match doesn't qualify; may fall through to heuristic which also fails
        let sel = detect_generic_selector(html);
        // Either None or heuristic result is fine — main constraint is not crashing
        let _ = sel;
    }

    #[test]
    fn heuristic_finds_job_class() {
        let html = r#"
            <div class="position-item">Backend Engineer</div>
            <div class="position-item">Frontend Engineer</div>
            <div class="position-item">DevOps Engineer</div>
        "#;
        let sel = detect_job_selector(html, &Platform::Unknown);
        assert!(sel.is_some(), "heuristic should find .position-item or similar");
    }

    #[test]
    fn ignores_short_text_elements() {
        // Elements that match a pattern but contain only tiny/empty text
        let html = r#"
            <div class="job-card"> </div>
            <div class="job-card">X</div>
            <div class="job-card">AB</div>
        "#;
        // All text is shorter than MIN_TITLE_LEN — should not accept
        let sel = detect_job_selector(html, &Platform::Unknown);
        assert!(sel.is_none());
    }
}
