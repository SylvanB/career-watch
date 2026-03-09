//! Binary entrypoint for the career-page discovery worker.
//!
//! ## What it does
//!
//! 1. **Seed** — fetches company leads from all configured seed sources.
//! 2. **Enqueue** — inserts new companies into the `companies` table (dedup by domain).
//! 3. **Probe pending** — probes each `status = 'pending'` company for a career page.
//! 4. **Re-probe stale** — re-checks `status = 'active'` companies whose
//!    `last_probed_at` is older than the configured interval.
//!
//! ## Usage
//!
//! ```sh
//! cargo run --bin career-watch-discovery
//! ```
//!
//! Designed to be run periodically via cron or a task scheduler:
//!
//! ```cron
//! # Run every 6 hours
//! 0 */6 * * * /path/to/career-watch-discovery >> /var/log/discovery.log 2>&1
//! ```

use anyhow::Result;
use career_watch::{
    config::AppConfig,
    db::create_pool,
    discovery::{
        prober::CareerProber,
        scheduler::{DiscoveryScheduler, build_http_client},
        sources::{CompanyLead, GithubOrgSource, SeedSource, StaticListSource},
    },
};
use tracing::info;
use tracing_subscriber::{EnvFilter, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .finish()
        .init();

    let config = AppConfig::from_env()?;
    let pool = create_pool(&config.database_url).await?;
    sqlx::migrate!().run(&pool).await?;

    let http = build_http_client()?;
    let prober = CareerProber::new()?;
    let scheduler = DiscoveryScheduler::new(pool, prober);

    // ── Phase 1: collect leads from all seed sources ──────────────────────────

    let sources: Vec<Box<dyn SeedSource>> = build_seed_sources();
    let mut all_leads: Vec<CompanyLead> = Vec::new();

    for source in &sources {
        info!(source = source.name(), "fetching seed source");
        match source.fetch(&http).await {
            Ok(leads) => {
                info!(source = source.name(), count = leads.len(), "received leads");
                all_leads.extend(leads);
            }
            Err(e) => {
                tracing::warn!(source = source.name(), error = %e, "seed source failed");
            }
        }
    }

    // ── Phase 2: enqueue new companies (dedup by domain) ─────────────────────

    let inserted = scheduler.enqueue_companies(all_leads).await?;
    info!(inserted, "companies enqueued");

    // ── Phase 3: probe pending companies ─────────────────────────────────────

    let pending = scheduler.pending_companies(200).await?;
    info!(count = pending.len(), "probing pending companies");
    scheduler.probe_all(pending).await?;

    // ── Phase 4: re-probe stale active companies ──────────────────────────────

    let stale = scheduler.stale_companies(100).await?;
    info!(count = stale.len(), "re-probing stale companies");
    scheduler.probe_all(stale).await?;

    info!("discovery run complete");
    Ok(())
}

/// Build the list of seed sources for this run.
///
/// Extend this function to add new sources. Sources are processed in order;
/// duplicate domains are automatically deduplicated by the scheduler.
fn build_seed_sources() -> Vec<Box<dyn SeedSource>> {
    vec![
        // ── Static seed list ─────────────────────────────────────────────────
        // A hand-curated list of well-known companies. Good starting point.
        Box::new(StaticListSource {
            companies: static_seed_companies(),
        }),

        // ── GitHub organisations ──────────────────────────────────────────────
        // Extracts the company website field from a GitHub org profile.
        Box::new(GithubOrgSource {
            org: "vercel".to_string(),
        }),
        Box::new(GithubOrgSource {
            org: "supabase".to_string(),
        }),
        Box::new(GithubOrgSource {
            org: "planetscale".to_string(),
        }),
        Box::new(GithubOrgSource {
            org: "fly-apps".to_string(),
        }),
        Box::new(GithubOrgSource {
            org: "railway-app".to_string(),
        }),

        // ── Startup directories ───────────────────────────────────────────────
        // Uncomment and configure selectors for directories you have access to.
        //
        // Box::new(StartupDirectorySource {
        //     url: "https://example-directory.com/companies".to_string(),
        //     company_link_selector: "a.company-link".to_string(),
        // }),
    ]
}

/// Hardcoded seed companies for bootstrapping.
///
/// Format: `(name, domain)`. Add freely — duplicates are harmless.
fn static_seed_companies() -> Vec<CompanyLead> {
    let entries = [
        // Infrastructure / developer tools
        ("Stripe", "stripe.com"),
        ("Vercel", "vercel.com"),
        ("Cloudflare", "cloudflare.com"),
        ("Datadog", "datadoghq.com"),
        ("HashiCorp", "hashicorp.com"),
        ("Grafana Labs", "grafana.com"),
        ("Fly.io", "fly.io"),
        ("Render", "render.com"),
        ("Railway", "railway.app"),
        ("Supabase", "supabase.com"),
        ("PlanetScale", "planetscale.com"),
        ("Neon", "neon.tech"),
        ("Turso", "turso.tech"),
        // AI / ML
        ("Anthropic", "anthropic.com"),
        ("OpenAI", "openai.com"),
        ("Mistral AI", "mistral.ai"),
        ("Cohere", "cohere.com"),
        ("Scale AI", "scale.com"),
        ("Hugging Face", "huggingface.co"),
        ("Replicate", "replicate.com"),
        // Fintech
        ("Plaid", "plaid.com"),
        ("Brex", "brex.com"),
        ("Ramp", "ramp.com"),
        ("Mercury", "mercury.com"),
        ("Rippling", "rippling.com"),
        // Collaboration / productivity
        ("Linear", "linear.app"),
        ("Notion", "notion.so"),
        ("Loom", "loom.com"),
        ("Retool", "retool.com"),
    ];

    entries
        .into_iter()
        .map(|(name, domain)| CompanyLead {
            name: name.to_string(),
            domain: domain.to_string(),
            source: "static_seed".to_string(),
        })
        .collect()
}
