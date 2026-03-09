//! LLM-assisted job extraction and selector detection via a local Ollama model.
//!
//! Two modes:
//!
//! 1. **Discovery** (`detect_selectors_via_llm`) — asks the model to identify
//!    CSS selectors AND whether the page actually contains job listings. When
//!    the page has no jobs but the model can see where they would be (e.g. an
//!    ATS embed URL in a script tag), it returns a `jobs_url_hint` for the
//!    pipeline to follow.
//!
//! 2. **Extraction** (`extract_jobs_via_llm`) — asks the model to extract the
//!    full list of jobs as structured JSON directly from the HTML. Used by the
//!    worker as the primary extraction method, with CSS selectors as fallback.
//!
//! ## Setup
//!
//! ```sh
//! # Native: https://ollama.ai/download
//! # or via Docker:
//! make ollama-up && make ollama-pull
//! ```
//!
//! `.env`:
//! ```
//! OLLAMA_HOST=http://localhost:11434
//! OLLAMA_MODEL=qwen2.5:3b
//! ```

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, info, warn};

use crate::discovery::selector::selector_hit_count;
use crate::models::JobPosting;

// ── Shared Ollama client ──────────────────────────────────────────────────────

fn ollama_client() -> Result<Client> {
    Ok(Client::builder()
        .timeout(Duration::from_secs(90)) // local models can be slow on first run
        .build()?)
}

#[derive(Serialize)]
struct OllamaChatRequest<'a> {
    model: &'a str,
    messages: Vec<OllamaMessage>,
    /// Omitted when null — Ollama rejects `"format": null` with a 400.
    #[serde(skip_serializing_if = "serde_json::Value::is_null")]
    format: serde_json::Value,
    stream: bool,
    options: OllamaOptions,
}

#[derive(Serialize)]
struct OllamaMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct OllamaOptions {
    temperature: f32,
    num_predict: u32,
    /// Limit context window — ReaderLM-v2 defaults to 512K which causes
    /// Ollama to allocate ~15GB KV cache. 16K is plenty for a career page.
    num_ctx: u32,
}

#[derive(Deserialize)]
struct OllamaResponse {
    message: OllamaResponseMessage,
}

#[derive(Deserialize)]
struct OllamaResponseMessage {
    content: String,
}

async fn ollama_chat(
    host: &str,
    model: &str,
    prompt: &str,
    format: serde_json::Value,
    max_tokens: u32,
) -> Result<String> {
    let client = ollama_client()?;
    let url = format!("{}/api/chat", host.trim_end_matches('/'));

    let req = OllamaChatRequest {
        model,
        messages: vec![OllamaMessage { role: "user".into(), content: prompt.into() }],
        format,
        stream: false,
        options: OllamaOptions { temperature: 0.1, num_predict: max_tokens, num_ctx: 16_384 },
    };

    let response = client
        .post(&url)
        .json(&req)
        .send()
        .await
        .context("failed to reach Ollama — is it running?")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Ollama returned {status} for model '{model}': {body}");
    }

    let resp: OllamaResponse = response
        .json()
        .await
        .context("failed to parse Ollama response")?;

    Ok(resp.message.content)
}

// ── HTML preprocessing ────────────────────────────────────────────────────────

/// Strip noise from HTML before sending to the LLM.
///
/// Removes `<script>`, `<style>`, `<svg>`, `<noscript>`, `<head>`, `<iframe>`
/// blocks entirely, collapses whitespace, and truncates to ~12k chars
/// (~3–5k tokens). Keeps all structural attributes (`class`, `id`, etc.)
/// intact so the LLM can reason about CSS selectors.
pub fn preprocess_html(html: &str) -> String {
    let mut text = html.to_string();

    for tag in &["script", "style", "svg", "noscript", "head", "iframe"] {
        let open = format!("<{}", tag);
        let close = format!("</{}>", tag);
        loop {
            let Some(start) = text.find(&open) else { break };
            if let Some(rel) = text[start..].find(&close) {
                text.replace_range(start..start + rel + close.len(), "");
            } else {
                text.replace_range(start.., "");
                break;
            }
        }
    }

    let mut result = String::with_capacity(text.len());
    let mut prev_ws = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !prev_ws { result.push(' '); prev_ws = true; }
        } else {
            result.push(ch);
            prev_ws = false;
        }
    }

    let body_start = result.find("<body").unwrap_or(0);
    let focused = &result[body_start..];

    const MAX_CHARS: usize = 12_000;
    if focused.len() > MAX_CHARS {
        let cut = focused[..MAX_CHARS].rfind('<').unwrap_or(MAX_CHARS);
        focused[..cut].to_string()
    } else {
        focused.to_string()
    }
}

// ── Mode 1: Discovery — selector detection + URL crawling hint ───────────────

/// Result of LLM-based selector detection during discovery.
#[derive(Debug, Clone)]
pub struct LlmSelectorResult {
    pub selector: String,
    pub title_within: Option<String>,
    pub location_within: Option<String>,
    pub confidence: String,
    /// When the current page has no job listings, the LLM may identify a
    /// different URL where jobs actually live (e.g. the ATS board URL extracted
    /// from an embedded script). The pipeline will follow this hint.
    pub jobs_url_hint: Option<String>,
}

#[derive(Deserialize)]
struct SelectorJson {
    selector: Option<String>,
    title_within: Option<String>,
    location_within: Option<String>,
    #[serde(default = "default_confidence")]
    confidence: String,
    jobs_url_hint: Option<String>,
}

fn default_confidence() -> String { "medium".to_string() }

/// Ask a local Ollama model to detect job listing selectors from page HTML.
///
/// Also looks for URL hints when the page has no inline job listings —
/// e.g. a SPA that loads from Greenhouse or Lever via a script tag.
pub async fn detect_selectors_via_llm(
    html: &str,
    page_url: &str,
    ollama_host: &str,
    model: &str,
) -> Result<Option<LlmSelectorResult>> {
    if ollama_host.is_empty() { return Ok(None); }

    let skeleton = preprocess_html(html);
    if skeleton.len() < 100 {
        debug!("HTML too short after preprocessing, skipping LLM");
        return Ok(None);
    }

    let prompt = format!(
        r#"Analyze this careers page HTML and return JSON only.

URL: {url}

Tasks:
1. Do individual job listings appear in this HTML?
2. If YES: identify CSS selectors for each job card, the title element within it, and the location element within it.
3. If NO: look for clues about WHERE jobs are loaded from. This could be:
   - An ATS board URL in a script tag (e.g. boards.greenhouse.io/company, jobs.lever.co/company)
   - A JSON config with a "for" or "company" key
   - Any URL pattern indicating where the job board is hosted

Return JSON only (no explanation):
{{
  "selector": "li.job-card",         // CSS selector for each job card; null if no jobs visible
  "title_within": "h3",              // selector for title inside card; null if not needed or no jobs
  "location_within": ".location",    // selector for location inside card; null if absent or no jobs
  "confidence": "high",              // high/medium/low/none
  "jobs_url_hint": null              // if no jobs: URL where jobs actually live, or null
}}

HTML:
{html}"#,
        url = page_url,
        html = skeleton,
    );

    let format = serde_json::json!({
        "type": "object",
        "properties": {
            "selector":        { "type": ["string", "null"] },
            "title_within":    { "type": ["string", "null"] },
            "location_within": { "type": ["string", "null"] },
            "confidence":      { "type": "string", "enum": ["high", "medium", "low", "none"] },
            "jobs_url_hint":   { "type": ["string", "null"] }
        },
        "required": ["selector", "confidence"]
    });

    let raw = match ollama_chat(ollama_host, model, &prompt, format, 200).await {
        Ok(s) => s,
        Err(e) => {
            warn!("Ollama selector detection failed: {e}");
            return Ok(None);
        }
    };

    let parsed: SelectorJson = match serde_json::from_str(&raw) {
        Ok(p) => p,
        Err(e) => {
            warn!("LLM returned invalid JSON for selector detection: {e} — raw: {raw}");
            return Ok(None);
        }
    };

    debug!(
        selector = ?parsed.selector,
        title_within = ?parsed.title_within,
        confidence = parsed.confidence,
        jobs_url_hint = ?parsed.jobs_url_hint,
        "LLM selector result"
    );

    // If a URL hint was found (no jobs on this page), return it even without a selector.
    if let Some(ref hint) = parsed.jobs_url_hint {
        if !hint.is_empty() && parsed.selector.as_deref().map_or(true, |s| s.is_empty()) {
            info!(hint, "LLM found jobs URL hint — will probe that URL");
            return Ok(Some(LlmSelectorResult {
                selector: String::new(),
                title_within: None,
                location_within: None,
                confidence: "hint".into(),
                jobs_url_hint: Some(hint.clone()),
            }));
        }
    }

    // Require a real selector with medium+ confidence.
    let selector = match (&parsed.selector, parsed.confidence.as_str()) {
        (Some(s), "high" | "medium") if !s.is_empty() => s.clone(),
        _ => return Ok(None),
    };

    // Validate: selector must match at least something in the live HTML.
    if selector_hit_count(html, &selector) == 0 {
        warn!(%selector, "LLM selector matches 0 elements on the live page");
        return Ok(None);
    }

    Ok(Some(LlmSelectorResult {
        selector,
        title_within: parsed.title_within.filter(|s| !s.is_empty()),
        location_within: parsed.location_within.filter(|s| !s.is_empty()),
        confidence: parsed.confidence,
        jobs_url_hint: parsed.jobs_url_hint,
    }))
}

// ── Mode 2: Extraction — direct job data from HTML ───────────────────────────

const DEPT_VALUES: &str = "engineering, product, sales, marketing, design, data, operations, people, finance, legal, customer_success, other";

/// Returns true when the model name identifies it as a ReaderLM variant.
/// ReaderLM uses a different prompt convention and wraps output in code fences.
fn is_reader_lm(model: &str) -> bool {
    let lower = model.to_lowercase();
    lower.contains("readerlm") || lower.contains("reader-lm") || lower.contains("reader_lm")
}

/// Strip code fences and normalise the JSON so it is always a bare array.
///
/// Models commonly return one of:
/// - A bare array:                `[{...}, ...]`               ← ideal
/// - Fenced bare array:           ` ```json\n[...]\n``` `      ← ReaderLM
/// - Object wrapping the array:   `{"items": [...]}` etc.      ← small models
/// - Truncated JSON:              `[{...}, {"title": "En`      ← long output
///
/// Returns the best attempt at a parseable array string.
fn strip_and_normalise_json(raw: &str) -> String {
    // 1. Strip ```json / ``` fences.
    let s = raw.trim();
    let s = s.strip_prefix("```json").or_else(|| s.strip_prefix("```"))
        .unwrap_or(s)
        .strip_suffix("```")
        .unwrap_or(s)
        .trim();

    // 2. Object wrapper: {"items": [...]} / {"jobs": [...]} / {"data": [...]} etc.
    if s.starts_with('{') {
        if let Ok(obj) = serde_json::from_str::<serde_json::Value>(s) {
            for key in &["items", "jobs", "results", "data", "listings", "openings", "positions"] {
                if let Some(arr) = obj.get(key) {
                    if arr.is_array() {
                        if let Ok(json) = serde_json::to_string(arr) {
                            return json;
                        }
                    }
                }
            }
            // First array-valued field wins.
            if let Some(obj_map) = obj.as_object() {
                for v in obj_map.values() {
                    if v.is_array() {
                        if let Ok(json) = serde_json::to_string(v) {
                            return json;
                        }
                    }
                }
            }
        }
    }

    // 3. Valid array — return as-is.
    if serde_json::from_str::<serde_json::Value>(s).map(|v| v.is_array()).unwrap_or(false) {
        return s.to_string();
    }

    // 4. Truncated array — salvage complete objects.
    if let Some(start) = s.find('[') {
        let fragment = &s[start..];
        let mut depth = 0i32;
        let mut last_complete = 0usize;
        for (i, ch) in fragment.char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 { last_complete = i + 1; }
                }
                _ => {}
            }
        }
        if last_complete > 0 {
            return format!("[{}]", &fragment[1..last_complete]);
        }
    }

    s.to_string()
}

/// Extract structured job listings directly from HTML using a local LLM.
///
/// Returns the full list of jobs on the page including title, location,
/// department, and URL. Called by the worker instead of CSS selector parsing.
///
/// Returns `Ok(None)` when Ollama is not configured or unreachable.
/// Returns `Ok(Some([]))` when the model confirms the page has no jobs.
pub async fn extract_jobs_via_llm(
    html: &str,
    page_url: &str,
    company_name: &str,
    ollama_host: &str,
    model: &str,
) -> Result<Option<Vec<JobPosting>>> {
    if ollama_host.is_empty() { return Ok(None); }

    let skeleton = preprocess_html(html);
    if skeleton.len() < 100 { return Ok(Some(vec![])); }

    let schema_json = serde_json::json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "title":      { "type": "string" },
                "location":   { "type": ["string", "null"] },
                "department": { "type": "string", "enum": DEPT_VALUES.split(", ").collect::<Vec<_>>() },
                "url":        { "type": ["string", "null"] }
            },
            "required": ["title", "department"]
        }
    });

    // ReaderLM-v2 uses a documented prompt convention:
    //   1. Instruction sentence
    //   2. HTML wrapped in ```html fences
    //   3. JSON schema wrapped in ```json fences
    // Output is also wrapped in ```json fences and must be stripped.
    //
    // General-purpose models (qwen2.5:3b etc) use a plain prose prompt.
    let (prompt, format) = if is_reader_lm(model) {
        let p = format!(
            "Extract the specified information from the HTML and present it in a structured JSON format.\n```html\n{html}\n```\nThe JSON schema is as follows:\n```json\n{schema}\n```",
            html   = skeleton,
            schema = serde_json::to_string_pretty(&schema_json).unwrap_or_default(),
        );
        // ReaderLM ignores the Ollama format field — it always outputs fenced JSON.
        // Pass null so Ollama doesn't apply conflicting constraints.
        (p, serde_json::Value::Null)
    } else {
        let p = format!(
            r#"Extract ALL job listings from this careers page HTML.

URL: {url}
Company: {company}

Return a JSON array. Each element must have:
- "title":      job title only (no location, no department suffix)
- "location":   city/country or "Remote" — null if not found
- "department": one of [{depts}]
- "url":        URL path or full URL to the job detail page — null if not found

If no job listings are visible, return an empty array: []
Return JSON array only. No explanation.

HTML:
{html}"#,
            url     = page_url,
            company = company_name,
            depts   = DEPT_VALUES,
            html    = skeleton,
        );
        (p, schema_json)
    };

    // 8192 gives headroom for large job boards (~50 jobs × ~80 tokens each).
    let raw = match ollama_chat(ollama_host, model, &prompt, format, 8192).await {
        Ok(s) => s,
        Err(e) => {
            warn!(company = company_name, "Ollama extraction failed: {e}");
            return Ok(None);
        }
    };

    #[derive(Deserialize)]
    struct LlmJob {
        title: String,
        location: Option<String>,
        department: Option<String>,
        url: Option<String>,
    }

    let clean = strip_and_normalise_json(&raw);

    let llm_jobs: Vec<LlmJob> = match serde_json::from_str(&clean) {
        Ok(v) => v,
        Err(e) => {
            warn!(company = company_name, "LLM extraction returned invalid JSON: {e} — raw: {}", &raw[..raw.len().min(300)]);
            return Ok(None);
        }
    };

    info!(company = company_name, count = llm_jobs.len(), "LLM extracted jobs");

    let jobs = llm_jobs
        .into_iter()
        .filter(|j| !j.title.trim().is_empty())
        .map(|j| JobPosting {
            title: j.title.trim().to_string(),
            company: company_name.to_string(),
            location: j.location.filter(|s| !s.is_empty()),
            department: j.department.filter(|s| !s.is_empty()),
            posted_text: None,
            url: j.url.filter(|s| !s.is_empty()),
        })
        .collect();

    Ok(Some(jobs))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_script_and_style_blocks() {
        let html = r#"
            <html>
            <head><style>body { color: red; }</style></head>
            <body>
                <script>console.log('hello');</script>
                <div class="job-card"><h3>Engineer</h3></div>
                <div class="job-card"><h3>Designer</h3></div>
            </body>
            </html>
        "#;
        let result = preprocess_html(html);
        assert!(!result.contains("console.log"), "script block should be removed");
        assert!(!result.contains("color: red"), "style block should be removed");
        assert!(result.contains("job-card"), "job card class should be kept");
        assert!(result.contains("Engineer"), "text content should be kept");
    }

    #[test]
    fn truncates_long_html() {
        let html = format!("<body>{}</body>", "x".repeat(20_000));
        let result = preprocess_html(&html);
        assert!(result.len() <= 12_100);
    }

    #[test]
    fn handles_missing_script_close_tag() {
        let html = "<body><script>no close tag <div class=\"job\">Engineer</div>";
        let result = preprocess_html(html);
        assert!(!result.contains("no close tag"));
    }

    #[test]
    fn prompt_includes_dept_values() {
        assert!(DEPT_VALUES.contains("engineering"));
        assert!(DEPT_VALUES.contains("customer_success"));
    }

    #[test]
    fn reader_lm_detection() {
        assert!(is_reader_lm("GFalcon-UA/ReaderLM-v2:Q8"));
        assert!(is_reader_lm("reader-lm-v2"));
        assert!(is_reader_lm("milkey/reader_lm-v2"));
        assert!(!is_reader_lm("qwen2.5:3b"));
        assert!(!is_reader_lm("llama3.2:3b"));
    }

    #[test]
    fn normalise_bare_array() {
        let r = strip_and_normalise_json(r#"[{"title":"Eng","department":"engineering"}]"#);
        assert!(r.starts_with('['));
    }

    #[test]
    fn normalise_fenced_array() {
        let r = strip_and_normalise_json("```json\n[{\"title\":\"Eng\",\"department\":\"engineering\"}]\n```");
        assert_eq!(r, "[{\"title\":\"Eng\",\"department\":\"engineering\"}]");
    }

    #[test]
    fn normalise_items_wrapper() {
        let r = strip_and_normalise_json(r#"{"items":[{"title":"Eng","department":"engineering"}]}"#);
        assert!(r.starts_with('['), "should unwrap items object: {r}");
    }

    #[test]
    fn normalise_jobs_wrapper() {
        let r = strip_and_normalise_json(r#"{"jobs":[{"title":"PM","department":"product"}]}"#);
        assert!(r.starts_with('['));
    }

    #[test]
    fn normalise_truncated_array() {
        // Simulates a cut-off response
        let raw = r#"[{"title":"Engineer","department":"engineering"},{"title":"Des"#;
        let r = strip_and_normalise_json(raw);
        // Should salvage the first complete object
        assert!(r.starts_with('['));
        let parsed: serde_json::Value = serde_json::from_str(&r).expect("should be valid JSON");
        assert_eq!(parsed.as_array().unwrap().len(), 1);
    }
}
