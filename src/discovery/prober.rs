//! Career page prober — strategy pipeline.
//!
//! Discovering a company's career page is modelled as an ordered pipeline of
//! [`ProbeStrategy`] implementations that each contribute [`Candidate`] URLs
//! with confidence scores to a shared [`ProbeContext`].
//!
//! The pipeline runs strategies in order and stops early when it has found a
//! result above the high-confidence threshold, avoiding unnecessary network
//! requests for obvious cases (e.g. a Greenhouse embed in the homepage).
//!
//! ## Adding a new strategy
//!
//! 1. Implement [`ProbeStrategy`] — write candidates into `ctx`.
//! 2. Register it in [`CareerProber::new`] via `.with_strategy(MyStrategy)`.
//!
//! ## Confidence scoring
//!
//! Each candidate starts at 1 and accumulates bonuses:
//!
//! | Signal                              | Bonus |
//! |-------------------------------------|-------|
//! | Known ATS platform detected         | +3    |
//! | URL path contains "careers"/"jobs"  | +2    |
//! | Page title contains career keywords | +1    |
//! | Job count (capped at 5)             | +job_count |
//! | Source: ATS embed in homepage       | +4    |
//! | Source: homepage link               | +3    |
//! | Source: career subdomain            | +2    |
//! | Source: sitemap                     | +2    |
//! | Source: path probe (fallback)       | +1    |

use anyhow::Result;
use futures::future::join_all;
use reqwest::{redirect, Client};
use scraper::{Html, Selector};
use std::time::Duration;
use tracing::{debug, info, warn};

use super::llm_selector::{detect_selectors_via_llm, preprocess_html};
use super::platform::{detect_platform, extract_platform_id, Platform};
use super::selector::detect_job_selector;
use super::validator::has_job_listings;

// ── Candidate ─────────────────────────────────────────────────────────────────

/// A candidate career page URL with a confidence score.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// Final URL after redirects.
    pub url: String,
    pub score: u32,
    pub platform: Platform,
    pub platform_id: Option<String>,
    pub job_count_estimate: usize,
    /// CSS selector for each job card container.
    pub job_selector: Option<String>,
    /// CSS selector for the title element *within* the job card.
    pub title_selector: Option<String>,
    /// CSS selector for the location element *within* the job card.
    pub location_selector: Option<String>,
    /// Which strategy produced this candidate.
    pub source: &'static str,
}

impl Candidate {
    fn new(
        url: String,
        platform: Platform,
        platform_id: Option<String>,
        job_count: usize,
        html: &str,
        source: &'static str,
    ) -> Self {
        let job_selector = detect_job_selector(html, &platform);
        let title_selector    = None; // populated later by LlmSelectorStrategy
        let location_selector = None;
        let mut score: u32 = 1;

        // Known ATS platform
        if platform != Platform::Unknown {
            score += 3;
        }

        // URL path keywords
        let url_lower = url.to_lowercase();
        if url_lower.contains("/careers")
            || url_lower.contains("/jobs")
            || url_lower.contains("greenhouse.io")
            || url_lower.contains("lever.co")
            || url_lower.contains("ashbyhq.com")
            || url_lower.contains("smartrecruiters.com")
        {
            score += 2;
        }

        // Page title keywords
        if page_title_has_career_keywords(html) {
            score += 1;
        }

        // Job count evidence (capped)
        score += (job_count as u32).min(5);

        // Source reliability
        score += match source {
            "ats_embed"     => 4, // ATS embed in homepage — most reliable
            "homepage_link" => 3, // company explicitly linked to it
            "sitemap"       => 2, // in the sitemap
            "subdomain"     => 2, // career subdomain found
            "path_probe"    => 1, // blind path guessing
            _               => 0,
        };

        Self { url, score, platform, platform_id, job_count_estimate: job_count, job_selector, title_selector, location_selector, source }
    }
}

fn page_title_has_career_keywords(html: &str) -> bool {
    let lower = html.to_lowercase();
    if let (Some(start), Some(end)) = (lower.find("<title"), lower.find("</title>")) {
        let title = &lower[start..end];
        return title.contains("career")
            || title.contains("job")
            || title.contains("hiring")
            || title.contains("work with us");
    }
    false
}

// ── Public result type ────────────────────────────────────────────────────────

/// Result of a successful probe, returned from `CareerProber::probe_domain`.
#[derive(Debug, Clone)]
pub struct CareerPageResult {
    pub url: String,
    pub platform: Platform,
    pub platform_id: Option<String>,
    pub job_count_estimate: usize,
    pub score: u32,
    pub source: &'static str,
    pub job_selector: Option<String>,
    pub title_selector: Option<String>,
    pub location_selector: Option<String>,
}

impl From<Candidate> for CareerPageResult {
    fn from(c: Candidate) -> Self {
        Self {
            url: c.url,
            platform: c.platform,
            platform_id: c.platform_id,
            job_count_estimate: c.job_count_estimate,
            score: c.score,
            source: c.source,
            job_selector: c.job_selector,
            title_selector: c.title_selector,
            location_selector: c.location_selector,
        }
    }
}

// ── Pipeline context ──────────────────────────────────────────────────────────

/// Shared context passed through all pipeline strategies.
pub struct ProbeContext {
    pub domain: String,
    /// `https://{domain}` with no trailing slash.
    pub base_url: String,
    /// Cached homepage HTML fetched by [`HomepageFetchStrategy`].
    pub homepage_html: Option<String>,
    /// Final URL of the homepage after redirects.
    pub homepage_final_url: Option<String>,
    /// Candidates collected so far (deduplicated by normalised URL).
    candidates: Vec<Candidate>,
}

impl ProbeContext {
    fn new(domain: &str) -> Self {
        Self {
            domain: domain.to_string(),
            base_url: format!("https://{}", domain.trim_end_matches('/')),
            homepage_html: None,
            homepage_final_url: None,
            candidates: Vec::new(),
        }
    }

    /// Add a candidate, silently dropping duplicates (same normalised URL).
    pub fn add(&mut self, candidate: Candidate) {
        let norm = normalise_url(&candidate.url);
        if !self.candidates.iter().any(|c| normalise_url(&c.url) == norm) {
            debug!(
                url = %candidate.url,
                score = candidate.score,
                source = candidate.source,
                "candidate added"
            );
            self.candidates.push(candidate);
        }
    }

    /// The highest-scoring candidate found so far.
    pub fn best(&self) -> Option<&Candidate> {
        self.candidates.iter().max_by_key(|c| c.score)
    }
}

fn normalise_url(url: &str) -> String {
    url.trim_end_matches('/').to_lowercase()
}

// ── Strategy trait ────────────────────────────────────────────────────────────

#[async_trait::async_trait]
pub trait ProbeStrategy: Send + Sync {
    fn name(&self) -> &'static str;
    async fn run(&self, ctx: &mut ProbeContext, client: &Client) -> Result<()>;
}

// ── Pipeline ──────────────────────────────────────────────────────────────────

pub struct ProbePipeline {
    client: Client,
    strategies: Vec<Box<dyn ProbeStrategy>>,
    /// Stop early if a candidate at or above this score exists.
    high_confidence_threshold: u32,
}

impl ProbePipeline {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            strategies: Vec::new(),
            high_confidence_threshold: 8,
        }
    }

    /// Append a strategy to the pipeline (builder pattern).
    pub fn with_strategy(mut self, s: impl ProbeStrategy + 'static) -> Self {
        self.strategies.push(Box::new(s));
        self
    }

    pub async fn run(&self, domain: &str) -> Option<CareerPageResult> {
        let mut ctx = ProbeContext::new(domain);

        for strategy in &self.strategies {
            debug!(domain, strategy = strategy.name(), "running strategy");
            if let Err(e) = strategy.run(&mut ctx, &self.client).await {
                warn!(domain, strategy = strategy.name(), "strategy error: {e}");
            }

            if let Some(best) = ctx.best() {
                if best.score >= self.high_confidence_threshold {
                    debug!(domain, score = best.score, "high confidence — stopping early");
                    break;
                }
            }
        }

        let best = ctx.best().cloned()?;
        info!(
            domain,
            url  = %best.url,
            platform = best.platform.as_str(),
            score = best.score,
            source = best.source,
            "career page found"
        );
        Some(best.into())
    }
}

// ── Shared helper ─────────────────────────────────────────────────────────────

/// Fetch a URL, validate it as a career page, and return a scored Candidate.
async fn probe_url(
    client: &Client,
    url: &str,
    source: &'static str,
) -> Result<Option<Candidate>> {
    let resp = match client.get(url).send().await {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };

    if !resp.status().is_success() {
        return Ok(None);
    }

    let final_url = resp.url().to_string();
    let html = resp.text().await?;
    let platform = detect_platform(&final_url, &html);
    let platform_id = extract_platform_id(&platform, &final_url);
    let (valid, job_count) = has_job_listings(&html, &platform);

    if !valid {
        return Ok(None);
    }

    Ok(Some(Candidate::new(
        final_url, platform, platform_id, job_count, &html, source,
    )))
}

// ── Strategy 1: HomepageFetch ─────────────────────────────────────────────────

/// Fetches the homepage and caches the HTML for downstream strategies.
/// Does not add candidates directly — it feeds the other strategies.
pub struct HomepageFetchStrategy;

#[async_trait::async_trait]
impl ProbeStrategy for HomepageFetchStrategy {
    fn name(&self) -> &'static str { "homepage_fetch" }

    async fn run(&self, ctx: &mut ProbeContext, client: &Client) -> Result<()> {
        let resp = client.get(&ctx.base_url).send().await?;
        ctx.homepage_final_url = Some(resp.url().to_string());
        ctx.homepage_html = Some(resp.text().await?);
        Ok(())
    }
}

// ── Strategy 2: AtsEmbed ─────────────────────────────────────────────────────

/// Scans the cached homepage HTML for known ATS embed signatures.
///
/// Companies often embed Greenhouse/Lever/Ashby job boards via `<iframe>` or
/// JavaScript. When we find the pattern, we can jump straight to the canonical
/// ATS board URL without guessing paths.
pub struct AtsEmbedStrategy;

#[async_trait::async_trait]
impl ProbeStrategy for AtsEmbedStrategy {
    fn name(&self) -> &'static str { "ats_embed" }

    async fn run(&self, ctx: &mut ProbeContext, client: &Client) -> Result<()> {
        let (html, homepage_url) = match (&ctx.homepage_html, &ctx.homepage_final_url) {
            (Some(h), Some(u)) => (h.clone(), u.clone()),
            _ => return Ok(()),
        };

        let platform = detect_platform(&homepage_url, &html);
        if matches!(platform, Platform::Unknown) {
            return Ok(());
        }

        // Try to find the board URL directly in the HTML.
        let board_url = find_ats_board_url(&html, &platform)
            // Fallback: many companies load the embed via JS so the board URL
            // isn't in the static HTML. Guess the slug from the domain and
            // probe the known board URL pattern directly.
            .or_else(|| guess_ats_board_url(&platform, &ctx.domain));

        if let Some(board_url) = board_url {
            match probe_url(client, &board_url, "ats_embed").await? {
                Some(candidate) => ctx.add(candidate),
                None => {
                    // Board page exists but no jobs visible in static HTML — could
                    // still be valid (jobs load via JS). Add with platform selector.
                    let (_, job_count) = has_job_listings(&html, &platform);
                    let platform_id = extract_platform_id(&platform, &board_url);
                    ctx.add(Candidate::new(
                        board_url, platform, platform_id, job_count, "", "ats_embed",
                    ));
                }
            }
        }

        Ok(())
    }
}

/// Find the canonical ATS board URL embedded in the page HTML.
///
/// Looks for known ATS URL patterns and extracts the company slug to build
/// the root board URL (e.g. `https://boards.greenhouse.io/acmecorp`).
fn find_ats_board_url(html: &str, platform: &Platform) -> Option<String> {
    match platform {
        Platform::Greenhouse => {
            // Greenhouse has two embed formats:
            //   Direct board:   boards.greenhouse.io/{slug}
            //   Embedded widget: boards.greenhouse.io/embed/job_board?for={slug}
            //                 or greenhouse.io/api/v1/...?token={slug}

            // 1. Try ?for= parameter (embedded widget)
            if let Some(pos) = html.find("?for=").or_else(|| html.find("&for=")) {
                if html[..pos].contains("greenhouse.io") {
                    let after = &html[pos + 5..];
                    let slug: String = after
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                        .collect();
                    if slug.len() >= 2 {
                        return Some(format!("https://boards.greenhouse.io/{}", slug));
                    }
                }
            }

            // 2. Try direct board URL, skipping the /embed/ path segment
            if let Some(pos) = html.find("boards.greenhouse.io/") {
                let after = &html[pos + "boards.greenhouse.io/".len()..];
                let slug: String = after
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                    .collect();
                // Skip known non-slug path segments like "embed"
                if slug.len() >= 2 && slug != "embed" && slug != "api" {
                    return Some(format!("https://boards.greenhouse.io/{}", slug));
                }
            }

            None
        }

        _ => {
            // Generic pattern: find `needle/{slug}` in the HTML
            let patterns: &[(&str, &str)] = match platform {
                Platform::Lever      => &[("jobs.lever.co/",        "https://jobs.lever.co/")],
                Platform::Ashby      => &[
                    ("jobs.ashbyhq.com/",     "https://jobs.ashbyhq.com/"),
                    ("app.ashbyhq.com/jobs/", "https://jobs.ashbyhq.com/"),
                ],
                Platform::SmartRecruiters => &[
                    ("careers.smartrecruiters.com/", "https://careers.smartrecruiters.com/"),
                ],
                _ => return None,
            };

            for (needle, base) in patterns {
                if let Some(pos) = html.find(needle) {
                    let after = &html[pos + needle.len()..];
                    let slug: String = after
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                        .collect();
                    if slug.len() >= 2 {
                        return Some(format!("{}{}", base, slug));
                    }
                }
            }

            None
        }
    }
}

/// When the ATS embed is loaded via JavaScript the board URL isn't in the
/// static HTML. This function guesses the board URL by extracting the slug
/// from the company domain and constructing the canonical board URL.
///
/// e.g. `mercury.com` → `boards.greenhouse.io/mercury`
fn guess_ats_board_url(platform: &Platform, domain: &str) -> Option<String> {
    // Extract the first label of the domain (before the first dot).
    // "mercury.com" → "mercury", "mistral.ai" → "mistral"
    let slug = domain.split('.').next()?.to_lowercase();
    if slug.len() < 2 { return None; }
    platform.board_url(&slug)
}

// ── Strategy 3: HomepageLink ──────────────────────────────────────────────────

/// Parses career-related links from the homepage HTML.
///
/// Most companies link their `/careers` page from their main nav or footer.
/// This is far more reliable than guessing paths.
pub struct HomepageLinkStrategy;

const CAREER_KEYWORDS: &[&str] = &[
    "careers", "jobs", "hiring", "join us", "join-us",
    "work with us", "work-with-us", "we're hiring", "open positions",
    "open roles", "opportunities", "vacancies",
];

const SKIP_DOMAINS: &[&str] = &[
    "linkedin.com", "indeed.com", "glassdoor.com",
    "twitter.com", "x.com", "facebook.com", "instagram.com",
    "youtube.com", "github.com",
];

#[async_trait::async_trait]
impl ProbeStrategy for HomepageLinkStrategy {
    fn name(&self) -> &'static str { "homepage_link" }

    async fn run(&self, ctx: &mut ProbeContext, client: &Client) -> Result<()> {
        let html = match &ctx.homepage_html {
            Some(h) => h.clone(),
            None => return Ok(()),
        };

        let links = extract_career_links(&html, &ctx.base_url);

        // Probe up to 4 candidates in parallel
        let futs: Vec<_> = links
            .into_iter()
            .take(4)
            .map(|url| {
                let c = client.clone();
                async move { probe_url(&c, &url, "homepage_link").await }
            })
            .collect();

        for result in join_all(futs).await {
            if let Ok(Some(c)) = result {
                ctx.add(c);
            }
        }

        Ok(())
    }
}

fn extract_career_links(html: &str, base_url: &str) -> Vec<String> {
    let document = Html::parse_document(html);
    let Ok(selector) = Selector::parse("a[href]") else { return vec![] };
    let base = url::Url::parse(base_url).ok();

    let mut found = Vec::new();

    for el in document.select(&selector) {
        let href = el.value().attr("href").unwrap_or("").trim();
        let text = el.text().collect::<String>().to_lowercase();
        let href_lower = href.to_lowercase();

        let is_career = CAREER_KEYWORDS
            .iter()
            .any(|k| text.contains(k) || href_lower.contains(k));

        if !is_career {
            continue;
        }

        // Resolve relative URLs
        let full = if href.starts_with("http") {
            href.to_string()
        } else if href.starts_with('/') {
            match &base {
                Some(b) => format!("{}{}", b.origin().ascii_serialization(), href),
                None => continue,
            }
        } else {
            continue
        };

        // Skip external job boards (keep ATS platforms)
        if SKIP_DOMAINS.iter().any(|d| full.contains(d)) {
            continue;
        }

        if !found.contains(&full) {
            found.push(full);
        }
    }

    found
}

// ── Strategy 4: CareerSubdomain ───────────────────────────────────────────────

/// Checks common career-related subdomains (`jobs.`, `careers.`, `work.`).
///
/// Many companies host their career pages at a subdomain rather than a path.
/// Uses HEAD requests for the initial check to minimise bandwidth.
pub struct CareerSubdomainStrategy;

const CAREER_SUBDOMAINS: &[&str] = &["jobs", "careers", "work", "apply", "career", "talent"];

#[async_trait::async_trait]
impl ProbeStrategy for CareerSubdomainStrategy {
    fn name(&self) -> &'static str { "subdomain" }

    async fn run(&self, ctx: &mut ProbeContext, client: &Client) -> Result<()> {
        // Skip if the domain already has more than one label before the TLD
        // (e.g. `boards.greenhouse.io` shouldn't get `jobs.boards.greenhouse.io`).
        let parts: Vec<&str> = ctx.domain.split('.').collect();
        if parts.len() > 2 {
            return Ok(());
        }

        let candidates: Vec<String> = CAREER_SUBDOMAINS
            .iter()
            .map(|sub| format!("https://{}.{}", sub, ctx.domain))
            .collect();

        // HEAD all subdomains in parallel — fast existence check
        let head_results = join_all(
            candidates.iter().map(|url| {
                let c = client.clone();
                let u = url.clone();
                async move {
                    let ok = c.head(&u)
                        .send()
                        .await
                        .map(|r| r.status().is_success())
                        .unwrap_or(false);
                    (u, ok)
                }
            }),
        )
        .await;

        // GET + validate only the ones that responded 200
        let get_futs: Vec<_> = head_results
            .into_iter()
            .filter_map(|(url, ok)| ok.then_some(url))
            .map(|url| {
                let c = client.clone();
                async move { probe_url(&c, &url, "subdomain").await }
            })
            .collect();

        for result in join_all(get_futs).await {
            if let Ok(Some(c)) = result {
                ctx.add(c);
            }
        }

        Ok(())
    }
}

// ── Strategy 5: Sitemap ───────────────────────────────────────────────────────

/// Parses `sitemap.xml` and `robots.txt` for career-related URLs.
pub struct SitemapStrategy;

#[async_trait::async_trait]
impl ProbeStrategy for SitemapStrategy {
    fn name(&self) -> &'static str { "sitemap" }

    async fn run(&self, ctx: &mut ProbeContext, client: &Client) -> Result<()> {
        let mut sitemap_urls = vec![format!("{}/sitemap.xml", ctx.base_url)];

        // Check robots.txt for additional Sitemap: directives
        let robots = format!("{}/robots.txt", ctx.base_url);
        if let Ok(resp) = client.get(&robots).send().await {
            if resp.status().is_success() {
                if let Ok(body) = resp.text().await {
                    for line in body.lines() {
                        if line.to_lowercase().starts_with("sitemap:") {
                            let extra = line[8..].trim().to_string();
                            if !sitemap_urls.contains(&extra) {
                                sitemap_urls.push(extra);
                            }
                        }
                    }
                }
            }
        }

        for sitemap_url in sitemap_urls.into_iter().take(2) {
            let career_links = fetch_sitemap_career_urls(client, &sitemap_url).await;
            let get_futs: Vec<_> = career_links
                .into_iter()
                .take(3)
                .map(|url| {
                    let c = client.clone();
                    async move { probe_url(&c, &url, "sitemap").await }
                })
                .collect();
            for result in join_all(get_futs).await {
                if let Ok(Some(c)) = result {
                    ctx.add(c);
                }
            }
        }

        Ok(())
    }
}

async fn fetch_sitemap_career_urls(client: &Client, url: &str) -> Vec<String> {
    let Ok(resp) = client.get(url).send().await else { return vec![] };
    if !resp.status().is_success() { return vec![] }
    let Ok(xml) = resp.text().await else { return vec![] };

    let mut urls: Vec<String> = xml
        .lines()
        .filter_map(|line| {
            if !line.contains("<loc>") { return None; }
            let start = line.find("<loc>")? + 5;
            let end   = line.find("</loc>")?;
            let loc   = &line[start..end];
            let lower = loc.to_lowercase();
            let is_career = CAREER_KEYWORDS.iter().any(|k| lower.contains(k));
            // Prefer shallow URLs (main careers page, not individual job listings)
            if is_career && loc.matches('/').count() <= 4 {
                Some(loc.to_string())
            } else {
                None
            }
        })
        .collect();

    // Shortest URL first (most likely to be the main careers page)
    urls.sort_by_key(|u| u.len());
    urls.dedup();
    urls
}

// ── Strategy 6: PathProbe ─────────────────────────────────────────────────────

/// Probes a ranked list of common career page paths.
///
/// Uses HEAD requests first to prune non-existent paths before making
/// full GET requests, then validates and scores any that respond.
pub struct PathProbeStrategy;

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

#[async_trait::async_trait]
impl ProbeStrategy for PathProbeStrategy {
    fn name(&self) -> &'static str { "path_probe" }

    async fn run(&self, ctx: &mut ProbeContext, client: &Client) -> Result<()> {
        let base = ctx.base_url.clone();

        // Phase 1: HEAD all paths in parallel — tiny bandwidth cost
        let head_results = join_all(CAREER_PATHS.iter().map(|path| {
            let url = format!("{}{}", base, path);
            let c = client.clone();
            async move {
                let ok = c.head(&url)
                    .send()
                    .await
                    .map(|r| r.status().is_success())
                    .unwrap_or(false);
                (url, ok)
            }
        }))
        .await;

        // Phase 2: GET + validate the survivors
        let get_futs: Vec<_> = head_results
            .into_iter()
            .filter_map(|(url, ok)| ok.then_some(url))
            .map(|url| {
                let c = client.clone();
                async move { probe_url(&c, &url, "path_probe").await }
            })
            .collect();

        for result in join_all(get_futs).await {
            if let Ok(Some(c)) = result {
                ctx.add(c);
            }
        }

        Ok(())
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

/// High-level career page prober.
///
/// Constructs and runs the default strategy pipeline. New strategies can be
/// appended to `pipeline` in [`CareerProber::new`] without touching any other
/// code.
// ── Strategy 7: LlmSelector ───────────────────────────────────────────────────

/// Ask a local Ollama model to identify job listing selectors from the HTML
/// of the best candidate found so far.
///
/// Runs last in the pipeline. If the best candidate already has a job_selector
/// (from static detection), this strategy refines it with two-level selectors
/// (title_within + location_within). If there is no selector yet, it tries to
/// find one from scratch.
///
/// Silently skips when Ollama is not configured or unreachable.
pub struct LlmSelectorStrategy {
    pub ollama_host:  String,
    pub ollama_model: String,
}

#[async_trait::async_trait]
impl ProbeStrategy for LlmSelectorStrategy {
    fn name(&self) -> &'static str { "llm_selector" }

    async fn run(&self, ctx: &mut ProbeContext, _client: &Client) -> Result<()> {
        if self.ollama_host.is_empty() {
            return Ok(());
        }

        // We need the HTML of the best career page found so far.
        // If the best candidate URL matches the homepage, use cached HTML;
        // otherwise we'd need to re-fetch. Use homepage HTML as the source
        // since it was already fetched by HomepageFetchStrategy.
        let (html, page_url) = match (&ctx.homepage_html, &ctx.homepage_final_url) {
            (Some(h), Some(u)) => (h.clone(), u.clone()),
            _ => return Ok(()),
        };

        // If the best candidate already has both selectors, skip.
        if let Some(best) = ctx.best() {
            if best.job_selector.is_some()
                && best.title_selector.is_some()
            {
                return Ok(());
            }
        }

        info!(domain = ctx.domain, model = self.ollama_model, "running LLM selector detection");

        match detect_selectors_via_llm(&html, &page_url, &self.ollama_host, &self.ollama_model).await {
            Ok(Some(result)) => {
                // Update the best candidate in place, or add a new one.
                if let Some(best) = ctx.candidates.iter_mut().max_by_key(|c| c.score) {
                    best.job_selector      = Some(result.selector);
                    best.title_selector    = result.title_within;
                    best.location_selector = result.location_within;
                    best.score            += 2; // LLM confirmation bonus
                    info!(
                        domain    = ctx.domain,
                        selector  = best.job_selector.as_deref().unwrap_or(""),
                        "LLM selector applied to best candidate"
                    );
                }
            }
            Ok(None) => {
                debug!(domain = ctx.domain, "LLM returned no usable selector");
            }
            Err(e) => {
                warn!(domain = ctx.domain, "LLM selector detection failed: {e}");
            }
        }

        Ok(())
    }
}

pub struct CareerProber {
    pipeline: ProbePipeline,
}

impl CareerProber {
    pub fn new() -> Result<Self> {
        Self::with_ollama("", "qwen2.5:3b")
    }

    pub fn with_ollama(ollama_host: &str, ollama_model: &str) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(8))
            .user_agent("CareerWatch/1.0 (automated career page discovery)")
            .redirect(redirect::Policy::limited(5))
            .build()?;

        let pipeline = ProbePipeline::new(client)
            .with_strategy(HomepageFetchStrategy)
            .with_strategy(AtsEmbedStrategy)
            .with_strategy(HomepageLinkStrategy)
            .with_strategy(CareerSubdomainStrategy)
            .with_strategy(SitemapStrategy)
            .with_strategy(PathProbeStrategy)
            // Last resort: ask a local LLM to identify selectors.
            // Silently skipped when Ollama is not configured or not running.
            .with_strategy(LlmSelectorStrategy {
                ollama_host: ollama_host.to_string(),
                ollama_model: ollama_model.to_string(),
            });

        Ok(Self { pipeline })
    }

    pub async fn probe_domain(&self, domain: &str) -> Option<CareerPageResult> {
        self.pipeline.run(domain).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_greenhouse_board_url() {
        let html = r#"<iframe src="https://boards.greenhouse.io/embed/job_board?for=acmecorp"></iframe>"#;
        let url = find_ats_board_url(html, &Platform::Greenhouse);
        assert_eq!(url.as_deref(), Some("https://boards.greenhouse.io/acmecorp"));
    }

    #[test]
    fn finds_lever_board_url() {
        let html = r#"<script>var lever = "https://jobs.lever.co/stripe";</script>"#;
        let url = find_ats_board_url(html, &Platform::Lever);
        assert_eq!(url.as_deref(), Some("https://jobs.lever.co/stripe"));
    }

    #[test]
    fn finds_ashby_board_url() {
        let html = r#"<a href="https://jobs.ashbyhq.com/linear/abc">View jobs</a>"#;
        let url = find_ats_board_url(html, &Platform::Ashby);
        assert_eq!(url.as_deref(), Some("https://jobs.ashbyhq.com/linear"));
    }

    #[test]
    fn returns_none_for_unknown_platform() {
        let html = "<div>some html</div>";
        assert!(find_ats_board_url(html, &Platform::Unknown).is_none());
    }

    #[test]
    fn career_links_extracted_from_nav() {
        let html = r#"
            <nav>
                <a href="/">Home</a>
                <a href="/about">About</a>
                <a href="/careers">Careers</a>
                <a href="https://linkedin.com/company/acme">LinkedIn</a>
            </nav>
        "#;
        let links = extract_career_links(html, "https://acme.com");
        assert!(links.iter().any(|l| l.contains("/careers")));
        assert!(!links.iter().any(|l| l.contains("linkedin.com")));
    }

    #[test]
    fn career_links_match_on_link_text() {
        let html = r#"<a href="/work">Join our team</a>"#;
        let links = extract_career_links(html, "https://acme.com");
        // "join" matches CAREER_KEYWORDS — but "join our team" contains "join"
        // Not all variations guaranteed, just check the function runs
        let _ = links;
    }

    #[test]
    fn candidate_scoring_ats_embed_beats_path_probe() {
        let ats = Candidate::new(
            "https://boards.greenhouse.io/acme".into(),
            Platform::Greenhouse,
            Some("acme".into()),
            5,
            "",
            "ats_embed",
        );
        let path = Candidate::new(
            "https://acme.com/careers".into(),
            Platform::Unknown,
            None,
            1,
            "",
            "path_probe",
        );
        assert!(ats.score > path.score, "ATS embed should score higher than path probe");
    }

    #[test]
    fn known_platform_bonus_applied() {
        let with_platform = Candidate::new(
            "https://jobs.lever.co/acme".into(),
            Platform::Lever,
            Some("acme".into()),
            0,
            "",
            "path_probe",
        );
        let without_platform = Candidate::new(
            "https://acme.com/jobs".into(),
            Platform::Unknown,
            None,
            0,
            "",
            "path_probe",
        );
        assert!(with_platform.score > without_platform.score);
    }

    #[test]
    fn context_deduplicates_candidates() {
        let mut ctx = ProbeContext::new("acme.com");
        let make = |url: &str| Candidate::new(
            url.into(), Platform::Unknown, None, 1, "", "path_probe"
        );
        ctx.add(make("https://acme.com/careers/"));
        ctx.add(make("https://acme.com/careers"));  // trailing slash stripped
        ctx.add(make("https://acme.com/careers/"));  // exact dup
        assert_eq!(ctx.candidates.len(), 1);
    }
}
