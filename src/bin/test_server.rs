//! Fake career page server for local end-to-end testing.
//!
//! Serves realistic career page HTML that the scraper, validator, and
//! platform detector all understand. Job listings are held in memory and
//! can be mutated at runtime via a simple admin API, allowing you to trigger
//! the change-detection logic in the worker without touching a real site.
//!
//! ## Start
//!
//! ```sh
//! make run-test-server
//! # Career page: http://localhost:9090/careers
//! # Admin API:   http://localhost:9090/admin/jobs
//! ```
//!
//! ## Create a watcher pointing at this server
//!
//! ```sh
//! curl -s -X POST http://localhost:8080/api/watches \
//!   -H 'Content-Type: application/json' \
//!   -d '{
//!     "company_name": "AcmeCorp (test)",
//!     "url": "http://localhost:9090/careers",
//!     "email": "you@example.com",
//!     "job_selector": "div.job-card"
//!   }'
//! ```
//!
//! ## Trigger a change
//!
//! ```sh
//! # Add a job
//! curl -s -X POST http://localhost:9090/admin/jobs \
//!   -H 'Content-Type: application/json' \
//!   -d '{"title":"Staff Engineer","department":"Platform","location":"Remote","job_type":"Full-time"}'
//!
//! # Remove a job by slug  (e.g. "senior-rust-engineer")
//! curl -s -X DELETE http://localhost:9090/admin/jobs/senior-rust-engineer
//!
//! # Reset to defaults
//! curl -s -X POST http://localhost:9090/admin/reset
//! ```
//!
//! Then run `make run-worker` — on the second run after any change you will
//! see a notification email arrive in Mailpit (http://localhost:8025).

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing_subscriber::{util::SubscriberInitExt, EnvFilter};

// ── Data model ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub title: String,
    pub department: String,
    pub location: String,
    pub job_type: String,
}

impl Job {
    /// URL-safe slug derived from the title, used as `data-job-id` and as
    /// the identifier for `DELETE /admin/jobs/:slug`.
    pub fn slug(&self) -> String {
        self.title
            .to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join("-")
    }

    /// Stable value for the `data-posted` attribute.
    ///
    /// Must NOT use Utc::now() — a fresh timestamp on every render would cause
    /// the scraper's posted_text to differ between runs, making every job look
    /// "new" even when nothing changed. The slug is stable across renders.
    pub fn posted_at(&self) -> String {
        self.slug()
    }
}

fn default_jobs() -> Vec<Job> {
    vec![
        Job {
            title: "Senior Rust Engineer".to_string(),
            department: "Engineering".to_string(),
            location: "Remote".to_string(),
            job_type: "Full-time".to_string(),
        },
        Job {
            title: "Product Designer".to_string(),
            department: "Design".to_string(),
            location: "New York, NY".to_string(),
            job_type: "Full-time".to_string(),
        },
        Job {
            title: "Backend Engineer".to_string(),
            department: "Engineering".to_string(),
            location: "San Francisco, CA".to_string(),
            job_type: "Full-time".to_string(),
        },
    ]
}

// ── Shared state ─────────────────────────────────────────────────────────────

type SharedJobs = Arc<RwLock<Vec<Job>>>;

// ── Main ─────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .finish()
        .init();

    let state: SharedJobs = Arc::new(RwLock::new(default_jobs()));

    let app = Router::new()
        // Career pages — prober checks all of these paths
        .route("/careers", get(career_page))
        .route("/jobs", get(career_page))
        .route("/join-us", get(career_page))
        // Admin API
        .route("/admin", get(admin_help))
        .route("/admin/jobs", get(list_jobs))
        .route("/admin/jobs", post(add_job))
        .route("/admin/jobs/:slug", delete(remove_job))
        .route("/admin/reset", post(reset_jobs))
        .with_state(state);

    let addr = "0.0.0.0:9090";
    tracing::info!("test career server listening on http://{}", addr);
    tracing::info!("career page  →  http://localhost:9090/careers");
    tracing::info!("admin UI     →  http://localhost:9090/admin");
    tracing::info!("admin API    →  http://localhost:9090/admin/jobs");

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

// ── Career page handlers ──────────────────────────────────────────────────────

async fn career_page(State(jobs): State<SharedJobs>) -> Html<String> {
    let jobs = jobs.read().await;
    Html(render_career_page(&jobs))
}

// ── Admin handlers ────────────────────────────────────────────────────────────

async fn admin_help() -> Html<&'static str> {
    Html(ADMIN_HELP_HTML)
}

async fn list_jobs(State(jobs): State<SharedJobs>) -> Json<Vec<Job>> {
    Json(jobs.read().await.clone())
}

async fn add_job(State(jobs): State<SharedJobs>, Json(job): Json<Job>) -> impl IntoResponse {
    let title = job.title.clone();
    jobs.write().await.push(job);
    tracing::info!(title, "job added");
    StatusCode::CREATED
}

async fn remove_job(State(jobs): State<SharedJobs>, Path(slug): Path<String>) -> impl IntoResponse {
    let mut jobs = jobs.write().await;
    let before = jobs.len();
    jobs.retain(|j| j.slug() != slug);

    if jobs.len() < before {
        tracing::info!(slug, "job removed");
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn reset_jobs(State(jobs): State<SharedJobs>) -> StatusCode {
    *jobs.write().await = default_jobs();
    tracing::info!("jobs reset to defaults");
    StatusCode::OK
}

// ── HTML rendering ────────────────────────────────────────────────────────────

fn render_career_page(jobs: &[Job]) -> String {
    // Each job card uses:
    //   class="job-card"           → matched by the validator's JOB_CARD_MARKERS
    //   data-job-id="{slug}"       → matched by the validator's JOB_CARD_MARKERS
    //   data-posted="{timestamp}"  → picked up by scraper's extract_jobs_from_html
    //
    // The full text content of each card becomes the job title in the scraper,
    // so the <h3> text is kept clean (no extra whitespace or nested text nodes
    // that would pollute it).
    //
    // Recommended job_selector when creating a watcher: "div.job-card h3.job-title"
    // This targets only the title element and produces clean job title strings.
    let cards: String = jobs
        .iter()
        .map(|j| {
            format!(
                r#"    <div class="job-card" data-job-id="{slug}" data-posted="{posted}">
      <h3 class="job-title">{title}</h3>
      <div class="job-meta">
        <span class="job-department">{dept}</span>
        <span class="job-location">{loc}</span>
        <span class="job-type">{jtype}</span>
      </div>
      <a href="/careers/{slug}/apply" class="apply-link">Apply now</a>
    </div>"#,
                slug = j.slug(),
                posted = j.posted_at(),
                title = html_escape(&j.title),
                dept = html_escape(&j.department),
                loc = html_escape(&j.location),
                jtype = html_escape(&j.job_type),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let count = jobs.len();
    let plural = if count == 1 { "" } else { "s" };

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Careers — AcmeCorp</title>
  <style>
    *, *::before, *::after {{ box-sizing: border-box; }}
    body {{
      font-family: system-ui, -apple-system, sans-serif;
      max-width: 860px; margin: 0 auto; padding: 2rem 1rem;
      color: #111; background: #fff;
    }}
    h1 {{ font-size: 2rem; margin-bottom: 0.25rem; }}
    .subtitle {{ color: #6b7280; margin-bottom: 2rem; }}
    .job-card {{
      border: 1px solid #e5e7eb; border-radius: 8px;
      padding: 1.25rem 1.5rem; margin: 1rem 0;
      transition: box-shadow 0.15s;
    }}
    .job-card:hover {{ box-shadow: 0 2px 8px rgba(0,0,0,.08); }}
    .job-title {{ margin: 0 0 0.5rem; font-size: 1.1rem; }}
    .job-meta {{ font-size: 0.85rem; color: #6b7280; display: flex; gap: 1rem; flex-wrap: wrap; }}
    .apply-link {{
      display: inline-block; margin-top: 1rem;
      padding: 0.4rem 1rem; border-radius: 4px;
      background: #2563eb; color: #fff; text-decoration: none;
      font-size: 0.875rem;
    }}
    .apply-link:hover {{ background: #1d4ed8; }}
  </style>
</head>
<body>
  <h1>Open Positions</h1>
  <p class="subtitle">{count} open role{plural} — come build with us.</p>
  <section id="job-listings">
{cards}
  </section>
</body>
</html>
"#,
        count = count,
        plural = plural,
        cards = cards,
    )
}

/// Minimal HTML escaping for user-supplied strings rendered into templates.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ── Admin help page ───────────────────────────────────────────────────────────

const ADMIN_HELP_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <title>Test Server — Admin</title>
  <style>
    body { font-family: monospace; max-width: 760px; margin: 2rem auto; padding: 0 1rem; line-height: 1.6; }
    h1 { font-family: system-ui, sans-serif; }
    h2 { margin-top: 2rem; border-bottom: 1px solid #e5e7eb; padding-bottom: 0.25rem; }
    pre { background: #f3f4f6; padding: 1rem; border-radius: 6px; overflow-x: auto; }
    code { background: #f3f4f6; padding: 0.1em 0.3em; border-radius: 3px; }
    a { color: #2563eb; }
  </style>
</head>
<body>
  <h1>Career Watch — Test Server</h1>
  <p>
    Serves a fake career page at <a href="/careers">/careers</a> and
    <a href="/jobs">/jobs</a>. Mutate the job list via the admin API to
    trigger change detection in the worker.
  </p>

  <h2>1. Create a watcher (run once)</h2>
  <pre>curl -s -X POST http://localhost:8080/api/watches \
  -H 'Content-Type: application/json' \
  -d '{
    "company_name": "AcmeCorp (test)",
    "url": "http://localhost:9090/careers",
    "email": "you@example.com",
    "job_selector": "div.job-card h3.job-title"
  }'</pre>

  <h2>2. Run the worker (establishes baseline)</h2>
  <pre>make run-worker</pre>

  <h2>3. Trigger a change</h2>

  <p><strong>Add a job:</strong></p>
  <pre>curl -s -X POST http://localhost:9090/admin/jobs \
  -H 'Content-Type: application/json' \
  -d '{
    "title": "Staff Engineer",
    "department": "Platform",
    "location": "Remote",
    "job_type": "Full-time"
  }'</pre>

  <p><strong>Remove a job</strong> (use the slug from the job title):</p>
  <pre>curl -s -X DELETE http://localhost:9090/admin/jobs/senior-rust-engineer</pre>

  <p><strong>List current jobs:</strong></p>
  <pre>curl -s http://localhost:9090/admin/jobs | jq</pre>

  <p><strong>Reset to defaults:</strong></p>
  <pre>curl -s -X POST http://localhost:9090/admin/reset</pre>

  <h2>4. Run the worker again — check Mailpit</h2>
  <pre>make run-worker
open http://localhost:8025</pre>
</body>
</html>
"#;
