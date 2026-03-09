//! Validates whether a fetched HTML page actually contains job listings.
//!
//! Uses keyword density analysis and structural heuristics. Platform-aware
//! shortcuts are applied when the page is hosted on a known ATS.

use super::platform::Platform;

/// Keywords that appear on genuine career pages.
const JOB_KEYWORDS: &[&str] = &[
    "apply",
    "engineer",
    "developer",
    "designer",
    "manager",
    "analyst",
    "full-time",
    "part-time",
    "remote",
    "opening",
    "position",
    "role",
    "vacancy",
    "opportunity",
];

/// Phrases that indicate a page explicitly states there are no openings.
const NO_OPENINGS_PHRASES: &[&str] = &[
    "no open positions",
    "no current openings",
    "no jobs available",
    "no openings at this time",
    "not currently hiring",
    "check back later",
    "no roles available",
];

/// CSS-class / data-attribute fragments commonly found on job cards.
const JOB_CARD_MARKERS: &[&str] = &[
    "job-card",
    "job_card",
    "jobcard",
    "position-item",
    "role-item",
    "opening-item",
    "job-listing",
    "job-row",
    "career-item",
    "data-job-id",
    "data-jobid",
    "data-job-slug",
];

/// Returns `(is_valid, estimated_job_count)`.
///
/// `is_valid` is `true` when the page looks like it contains job listings.
/// `estimated_job_count` is a rough count based on structural markers.
pub fn has_job_listings(html: &str, platform: &Platform) -> (bool, usize) {
    let lower = html.to_lowercase();

    // Explicit "not hiring" phrases take priority.
    for phrase in NO_OPENINGS_PHRASES {
        if lower.contains(phrase) {
            return (false, 0);
        }
    }

    // For known platforms, presence of the page is sufficient evidence —
    // the platform always renders jobs even if the list is temporarily empty.
    match platform {
        Platform::Greenhouse | Platform::Lever | Platform::Ashby | Platform::SmartRecruiters => {
            let count = count_job_card_markers(&lower);
            return (true, count.max(1));
        }
        _ => {}
    }

    // Generic heuristic: require a minimum keyword hit count AND either
    // list items or recognisable job-card markup.
    let keyword_hits: usize = JOB_KEYWORDS
        .iter()
        .map(|kw| lower.matches(kw).count())
        .sum();

    let li_count = lower.matches("<li").count();
    let card_count = count_job_card_markers(&lower);

    // Job-card markup alone is strong evidence; keywords + list items as fallback.
    let valid = card_count > 0 || (keyword_hits >= 3 && li_count > 3);
    let estimate = card_count.max(li_count / 5);

    (valid, estimate)
}

fn count_job_card_markers(html: &str) -> usize {
    JOB_CARD_MARKERS
        .iter()
        .map(|marker| html.matches(marker).count())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_no_openings_page() {
        let html = "<p>We have no open positions right now. Check back later.</p>";
        let (valid, _) = has_job_listings(html, &Platform::Unknown);
        assert!(!valid);
    }

    #[test]
    fn accepts_greenhouse_page_unconditionally() {
        let html = "<html><body>Loading...</body></html>";
        let (valid, _) = has_job_listings(html, &Platform::Greenhouse);
        assert!(valid);
    }

    #[test]
    fn counts_job_card_markers() {
        let html = r#"
            <div class="job-card">Engineer</div>
            <div class="job-card">Designer</div>
        "#;
        let (valid, count) = has_job_listings(html, &Platform::Unknown);
        assert!(valid);
        assert!(count >= 2);
    }
}
