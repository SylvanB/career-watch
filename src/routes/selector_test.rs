//! Selector test endpoint.
//!
//! POST /api/selector-test
//!
//! Fetches a URL and applies a CSS selector, returning the match count and
//! a sample of extracted titles. Used by the dashboard edit modal so users
//! can verify a selector works before saving it.

use axum::{extract::State, http::StatusCode, Extension, Json};
use serde::{Deserialize, Serialize};

use crate::discovery::selector::{selector_hit_count, selector_sample};
use crate::models::AuthUser;
use crate::AppState;

#[derive(Deserialize)]
pub struct SelectorTestRequest {
    pub url: String,
    pub selector: String,
}

#[derive(Serialize)]
pub struct SelectorTestResponse {
    /// Number of elements the selector matched.
    pub count: usize,
    /// Up to 5 extracted text strings from matched elements.
    pub sample: Vec<String>,
    /// Human-readable verdict.
    pub verdict: &'static str,
}

pub async fn test_selector(
    State(state): State<AppState>,
    Extension(_user): Extension<AuthUser>,
    Json(payload): Json<SelectorTestRequest>,
) -> Result<Json<SelectorTestResponse>, (StatusCode, Json<serde_json::Value>)> {
    let err = |msg: &str| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": msg })),
        )
    };

    if payload.url.is_empty() {
        return Err(err("url is required"));
    }
    if payload.selector.is_empty() {
        return Err(err("selector is required"));
    }

    // Fetch the page using the existing scraper client
    let html = state
        .scraper
        .fetch_raw_html(&payload.url)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": format!("could not fetch URL: {e}") })),
            )
        })?;

    let count  = selector_hit_count(&html, &payload.selector);
    let sample = selector_sample(&html, &payload.selector, 5);

    let verdict = match count {
        0          => "no matches — selector may be wrong or page uses JavaScript rendering",
        1          => "only 1 match — may work but a broader selector could be better",
        2..=5      => "good — found a small number of jobs",
        6..=50     => "great — found a healthy number of job listings",
        _          => "found many elements — selector may be too broad",
    };

    Ok(Json(SelectorTestResponse { count, sample, verdict }))
}
