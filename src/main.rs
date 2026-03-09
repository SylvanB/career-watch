//! Binary entrypoint for the Career Watch HTTP API server.

use std::net::SocketAddr;
use std::sync::Arc;

use career_watch::billing::StripeClient;
use career_watch::config::AppConfig;
use career_watch::db::create_pool;
use career_watch::email::EmailClient;
use career_watch::routes;
use career_watch::scraper::PageScraper;
use career_watch::AppState;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

#[tokio::main]
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
    let stripe = StripeClient::new(config.stripe_secret_key.clone())?;
    let config = Arc::new(config);

    let state = AppState { db, email, scraper, stripe, config: config.clone() };

    let addr: SocketAddr = config.bind_addr.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;

    tracing::info!("listening on {addr}");

    axum::serve(listener, routes::router(state)).await?;

    Ok(())
}
