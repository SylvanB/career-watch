//! Platform detection for major ATS / recruiting platforms.
//!
//! Identifies whether a career page is hosted on Greenhouse, Lever, Workday,
//! SmartRecruiters, Ashby, or a custom site. Detection is done by matching
//! against known URL patterns and HTML signatures embedded in the page.

/// Known recruiting platform identifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Platform {
    Greenhouse,
    Lever,
    Workday,
    SmartRecruiters,
    Ashby,
    BambooHr,
    Rippling,
    Unknown,
}

impl Platform {
    pub fn as_str(&self) -> &'static str {
        match self {
            Platform::Greenhouse => "greenhouse",
            Platform::Lever => "lever",
            Platform::Workday => "workday",
            Platform::SmartRecruiters => "smartrecruiters",
            Platform::Ashby => "ashby",
            Platform::BambooHr => "bamboohr",
            Platform::Rippling => "rippling",
            Platform::Unknown => "unknown",
        }
    }

    /// Canonical job board URL for platform-hosted companies, given their slug.
    pub fn board_url(&self, company_id: &str) -> Option<String> {
        match self {
            Platform::Greenhouse => {
                Some(format!("https://boards.greenhouse.io/{}", company_id))
            }
            Platform::Lever => Some(format!("https://jobs.lever.co/{}", company_id)),
            Platform::Ashby => Some(format!("https://jobs.ashbyhq.com/{}", company_id)),
            Platform::SmartRecruiters => {
                Some(format!("https://careers.smartrecruiters.com/{}", company_id))
            }
            _ => None,
        }
    }
}

/// URL substrings that reliably identify a platform.
const URL_PATTERNS: &[(&str, Platform)] = &[
    ("boards.greenhouse.io", Platform::Greenhouse),
    ("jobs.lever.co", Platform::Lever),
    ("myworkdayjobs.com", Platform::Workday),
    ("careers.smartrecruiters.com", Platform::SmartRecruiters),
    ("jobs.ashbyhq.com", Platform::Ashby),
    ("app.ashbyhq.com", Platform::Ashby),
    ("app.bamboohr.com", Platform::BambooHr),
    ("rippling.com/jobs", Platform::Rippling),
];

/// HTML content signatures (e.g. embedded iframes, script src attributes).
const HTML_SIGNATURES: &[(&str, Platform)] = &[
    ("greenhouse.io/embed", Platform::Greenhouse),
    ("boards.greenhouse.io", Platform::Greenhouse),
    ("jobs.lever.co", Platform::Lever),
    ("lever-jobs-embed", Platform::Lever),
    ("myworkdayjobs.com", Platform::Workday),
    ("careers.smartrecruiters.com", Platform::SmartRecruiters),
    ("jobs.ashbyhq.com", Platform::Ashby),
    ("ashbyhq.com", Platform::Ashby),
];

/// Detect which recruiting platform a page belongs to.
///
/// Checks the final URL (after redirects) first, then falls back to HTML
/// content inspection.
pub fn detect_platform(url: &str, html: &str) -> Platform {
    for (pattern, platform) in URL_PATTERNS {
        if url.contains(pattern) {
            return platform.clone();
        }
    }

    let html_lower = html.to_lowercase();
    for (sig, platform) in HTML_SIGNATURES {
        if html_lower.contains(sig) {
            return platform.clone();
        }
    }

    Platform::Unknown
}

/// Extract the company slug from a known platform URL.
///
/// Returns `None` for platforms where a slug cannot be reliably extracted,
/// or when the URL does not match the expected pattern.
pub fn extract_platform_id(platform: &Platform, url: &str) -> Option<String> {
    let segment_after = |needle: &str| -> Option<String> {
        url.split_once(needle)
            .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or(""))
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    };

    match platform {
        Platform::Greenhouse => segment_after("boards.greenhouse.io/"),
        Platform::Lever => segment_after("jobs.lever.co/"),
        Platform::Ashby => {
            segment_after("jobs.ashbyhq.com/").or_else(|| segment_after("app.ashbyhq.com/jobs/"))
        }
        Platform::SmartRecruiters => segment_after("careers.smartrecruiters.com/"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_greenhouse_by_url() {
        let p = detect_platform("https://boards.greenhouse.io/acme", "");
        assert_eq!(p, Platform::Greenhouse);
    }

    #[test]
    fn detects_lever_by_url() {
        let p = detect_platform("https://jobs.lever.co/stripe", "");
        assert_eq!(p, Platform::Lever);
    }

    #[test]
    fn detects_ashby_by_html() {
        let html = r#"<iframe src="https://jobs.ashbyhq.com/company/embed"></iframe>"#;
        let p = detect_platform("https://company.com/careers", html);
        assert_eq!(p, Platform::Ashby);
    }

    #[test]
    fn extracts_greenhouse_id() {
        let id = extract_platform_id(
            &Platform::Greenhouse,
            "https://boards.greenhouse.io/acmecorp",
        );
        assert_eq!(id.as_deref(), Some("acmecorp"));
    }

    #[test]
    fn extracts_lever_id() {
        let id = extract_platform_id(&Platform::Lever, "https://jobs.lever.co/stripe/abc");
        assert_eq!(id.as_deref(), Some("stripe"));
    }

    #[test]
    fn detects_workday_by_url() {
        let p = detect_platform("https://acme.myworkdayjobs.com/careers", "");
        assert_eq!(p, Platform::Workday);
    }

    #[test]
    fn detects_smartrecruiters_by_url() {
        let p = detect_platform("https://careers.smartrecruiters.com/Acme", "");
        assert_eq!(p, Platform::SmartRecruiters);
    }

    #[test]
    fn detects_bamboohr_by_url() {
        let p = detect_platform("https://app.bamboohr.com/jobs/acme", "");
        assert_eq!(p, Platform::BambooHr);
    }

    #[test]
    fn detects_greenhouse_by_html_embed() {
        let html = r#"<iframe src="https://boards.greenhouse.io/embed/acmecorp"></iframe>"#;
        let p = detect_platform("https://acme.com/careers", html);
        assert_eq!(p, Platform::Greenhouse);
    }

    #[test]
    fn returns_unknown_for_generic_page() {
        let p = detect_platform("https://example.com/careers", "<h1>Jobs</h1>");
        assert_eq!(p, Platform::Unknown);
    }

    #[test]
    fn url_pattern_takes_priority_over_html() {
        // URL clearly says Greenhouse, HTML says Lever — URL wins
        let html = r#"<a href="https://jobs.lever.co/acme">Apply</a>"#;
        let p = detect_platform("https://boards.greenhouse.io/acme", html);
        assert_eq!(p, Platform::Greenhouse);
    }

    #[test]
    fn extracts_ashby_id() {
        let id = extract_platform_id(
            &Platform::Ashby,
            "https://jobs.ashbyhq.com/acmecorp/role-slug",
        );
        assert_eq!(id.as_deref(), Some("acmecorp"));
    }

    #[test]
    fn extracts_smartrecruiters_id() {
        let id = extract_platform_id(
            &Platform::SmartRecruiters,
            "https://careers.smartrecruiters.com/AcmeCorp",
        );
        assert_eq!(id.as_deref(), Some("AcmeCorp"));
    }

    #[test]
    fn board_url_generates_correct_links() {
        assert_eq!(
            Platform::Greenhouse.board_url("acme"),
            Some("https://boards.greenhouse.io/acme".to_string())
        );
        assert_eq!(
            Platform::Lever.board_url("stripe"),
            Some("https://jobs.lever.co/stripe".to_string())
        );
        assert_eq!(Platform::Workday.board_url("acme"), None);
        assert_eq!(Platform::Unknown.board_url("acme"), None);
    }
}
