//! Core library crate for the Career Watch service.

pub mod auth;
pub mod billing;
pub mod config;
pub mod db;
pub mod discovery;
pub mod email;
pub mod models;
pub mod routes;
pub mod scraper;

use std::sync::Arc;

use billing::StripeClient;
use config::AppConfig;
use email::EmailClient;
use scraper::PageScraper;
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub email: EmailClient,
    pub scraper: PageScraper,
    pub stripe: StripeClient,
    pub config: Arc<AppConfig>,
}
