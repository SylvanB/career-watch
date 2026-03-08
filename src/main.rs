//! Binary entrypoint for the Career Watch HTTP API server.
//!
//! Wires together configuration, database, email client, and router and
//! starts an Axum server listening on `BIND_ADDR`.

use career_watch::config::AppConfig;
use career_watch::db::create_pool;
use career_watch::email::EmailClient;
use career_watch::routes;
use career_watch::scraper::PageScraper;
use career_watch::AppState;
use std::net::SocketAddr;
use tower_http::trace::TraceLayer;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

#[tokio::main]
/// Start the HTTP API server.
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .finish()
        .init();

    let config = AppConfig::from_env()?;
    let db = create_pool(&config.database_url).await?;
    sqlx::migrate!().run(&db).await?;

    let email = EmailClient::from_config(&config)?;
    let scraper = PageScraper::new();

    let state = AppState { db, email, scraper };

    let app = routes::router()
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr: SocketAddr = config.bind_addr.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;

    tracing::info!("listening on {}", addr);

    axum::serve(listener, app).await?;

    Ok(())
}

