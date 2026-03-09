//! LLM-assisted CSS selector detection via a local Ollama model.
//!
//! Sends a stripped DOM skeleton to a locally-running Ollama model and asks
//! it to identify the CSS selectors for job listing elements. Falls back
//! gracefully when Ollama is not running.
//!
//! ## Setup
//!
//! ```sh
//! # Install Ollama: https://ollama.ai
//! ollama pull qwen2.5:3b          # recommended — reliable JSON output
//! # or: ollama pull jinaai/reader-lm-v2  # purpose-built for HTML
//! ```
//!
//! Set in `.env`:
//! ```
//! OLLAMA_HOST=http://localhost:11434
//! OLLAMA_MODEL=qwen2.5:3b
//! ```

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, warn};

use crate::discovery::selector::selector_hit_count;

// ── Public result type ────────────────────────────────────────────────────────

/// Selectors detected by the LLM for a career page.
#[derive(Debug, Clone)]
pub struct LlmSelectorResult {
    /// CSS selector for the job card container element.
    pub selector: String,
    /// CSS selector for the title within the card (`h3`, `a`, etc.). `None` = use card text.
    pub title_within: Option<String>,
    /// CSS selector for the location within the card. `None` = no location element.
    pub location_within: Option<String>,
    pub confidence: String,
}

// ── Ollama API types ──────────────────────────────────────────────────────────

#[derive(Serialize)]
struct OllamaRequest<'a> {
    model: &'a str,
    messages: Vec<OllamaMessage<'a>>,
    format: serde_json::Value,
    stream: bool,
    options: OllamaOptions,
}

#[derive(Serialize)]
struct OllamaMessage<'a> {
    role: &'a str,
    content: String,
}

#[derive(Serialize)]
struct OllamaOptions {
    temperature: f32,
    num_predict: u32,
}

#[derive(Deserialize)]
struct OllamaResponse {
    message: OllamaResponseMessage,
}

#[derive(Deserialize)]
struct OllamaResponseMessage {
    content: String,
}

#[derive(Deserialize)]
struct SelectorJson {
    selector: Option<String>,
    title_within: Option<String>,
    location_within: Option<String>,
    #[serde(default = "default_confidence")]
    confidence: String,
}

fn default_confidence() -> String { "medium".to_string() }

// ── HTML preprocessing ────────────────────────────────────────────────────────

/// Strip noise from HTML before sending to the LLM.
///
/// Removes `<script>`, `<style>`, `<svg>`, `<noscript>` blocks,
/// collapses whitespace, and truncates to keep the token count manageable.
pub fn preprocess_html(html: &str) -> String {
    let mut text = html.to_string();

    // Remove block-level noise tags and all their content.
    for tag in &["script", "style", "svg", "noscript", "head", "iframe"] {
        let open = format!("<{}", tag);
        let close = format!("</{}>", tag);
        loop {
            let Some(start) = text.find(&open) else { break };
            // Find the matching close tag; if missing, cut from open tag onward.
            if let Some(rel) = text[start..].find(&close) {
                text.replace_range(start..start + rel + close.len(), "");
            } else {
                text.replace_range(start.., "");
                break;
            }
        }
    }

    // Collapse whitespace — keep all attributes intact so the LLM can
    // reason about class names, ids, and data attributes for selector detection.
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

    // Focus on the body content; skip HTML before <body> if present.
    let body_start = result.find("<body").unwrap_or(0);
    let focused = &result[body_start..];

    // Truncate — ~12k chars is roughly 3–5k tokens, well within any model's context.
    const MAX_CHARS: usize = 12_000;
    if focused.len() > MAX_CHARS {
        // Try to cut at a tag boundary.
        let cut = focused[..MAX_CHARS].rfind('<').unwrap_or(MAX_CHARS);
        focused[..cut].to_string()
    } else {
        focused.to_string()
    }
}

// ── Prompt ────────────────────────────────────────────────────────────────────

fn build_prompt(html_skeleton: &str, page_url: &str) -> String {
    format!(
        r#"You are analysing the HTML of a company careers/jobs page.
URL: {url}

Your task: identify the CSS selectors that target individual job listing elements.

Rules:
- "selector" must target each JOB CARD CONTAINER (not a wrapper around all jobs).
- "title_within" must be a CSS selector RELATIVE TO the card that contains ONLY the job title text.
- "location_within" must be a CSS selector RELATIVE TO the card that contains ONLY the location text. null if location is in a separate sibling element or not present.
- If no job listings are visible in the HTML (e.g. the page is a SPA loading screen), set selector to null.
- Respond with valid JSON only. No explanation.

Example response for a Greenhouse board:
{{"selector": "div.opening", "title_within": "a", "location_within": null, "confidence": "high"}}

Example response for a custom page:
{{"selector": "li.job-card", "title_within": "h3.job-title", "location_within": "span.location", "confidence": "medium"}}

HTML:
{html}"#,
        url = page_url,
        html = html_skeleton,
    )
}

// ── Main detection function ───────────────────────────────────────────────────

/// Ask a local Ollama model to detect job listing selectors from page HTML.
///
/// Returns `Ok(None)` when:
/// - Ollama host is not configured
/// - Ollama is not running
/// - The model returns no usable selector
/// - The model returns low/none confidence
pub async fn detect_selectors_via_llm(
    html: &str,
    page_url: &str,
    ollama_host: &str,
    model: &str,
) -> Result<Option<LlmSelectorResult>> {
    if ollama_host.is_empty() {
        return Ok(None);
    }

    let skeleton = preprocess_html(html);
    if skeleton.len() < 100 {
        debug!("HTML too short after preprocessing, skipping LLM");
        return Ok(None);
    }

    let prompt = build_prompt(&skeleton, page_url);

    let client = Client::builder()
        .timeout(Duration::from_secs(60)) // local models can be slow on first run
        .build()?;

    // JSON schema for constrained structured output (Ollama ≥ 0.5.0).
    let format = serde_json::json!({
        "type": "object",
        "properties": {
            "selector":        { "type": ["string", "null"] },
            "title_within":    { "type": ["string", "null"] },
            "location_within": { "type": ["string", "null"] },
            "confidence":      { "type": "string", "enum": ["high", "medium", "low", "none"] }
        },
        "required": ["selector", "confidence"]
    });

    let request = OllamaRequest {
        model,
        messages: vec![OllamaMessage { role: "user", content: prompt }],
        format,
        stream: false,
        options: OllamaOptions { temperature: 0.1, num_predict: 150 },
    };

    let url = format!("{}/api/chat", ollama_host.trim_end_matches('/'));

    let response = client
        .post(&url)
        .json(&request)
        .send()
        .await
        .context("failed to reach Ollama — is it running?")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        warn!(ollama_url = url, %status, "Ollama returned error: {body}");
        return Ok(None);
    }

    let ollama_resp: OllamaResponse = response.json().await
        .context("failed to parse Ollama response")?;

    let parsed: SelectorJson = serde_json::from_str(&ollama_resp.message.content)
        .context("LLM response was not valid JSON")?;

    debug!(
        selector = ?parsed.selector,
        title_within = ?parsed.title_within,
        location_within = ?parsed.location_within,
        confidence = parsed.confidence,
        "LLM selector result"
    );

    // Reject if no selector or low/none confidence.
    let selector = match (&parsed.selector, parsed.confidence.as_str()) {
        (Some(s), "high" | "medium") if !s.is_empty() => s.clone(),
        _ => return Ok(None),
    };

    // Validate the selector actually matches something in the HTML.
    let hits = selector_hit_count(html, &selector);
    if hits == 0 {
        warn!(%selector, "LLM selector matches 0 elements on the live page");
        return Ok(None);
    }

    debug!(%selector, hits, "LLM selector validated");

    Ok(Some(LlmSelectorResult {
        selector,
        title_within: parsed.title_within.filter(|s| !s.is_empty()),
        location_within: parsed.location_within.filter(|s| !s.is_empty()),
        confidence: parsed.confidence,
    }))
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
        assert!(result.len() <= 12_100, "should be truncated");
    }

    #[test]
    fn handles_missing_script_close_tag() {
        let html = "<body><script>no close tag <div class=\"job\">Engineer</div>";
        let result = preprocess_html(html);
        // Script content should be removed; job div may or may not survive
        assert!(!result.contains("no close tag"));
    }

    #[test]
    fn prompt_includes_url_and_html() {
        let prompt = build_prompt("<div class=\"job\">Eng</div>", "https://example.com/jobs");
        assert!(prompt.contains("https://example.com/jobs"));
        assert!(prompt.contains("job"));
    }
}
