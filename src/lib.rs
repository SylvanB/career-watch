//! Core library crate for the Career Watch service.
//!
//! This crate exposes shared modules and the `AppState` used by both the
//! HTTP API binary and the background worker.

pub mod config;
pub mod db;
pub mod email;
pub mod models;
pub mod routes;
pub mod scraper;

use email::EmailClient;
use scraper::PageScraper;
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub email: EmailClient,
    pub scraper: PageScraper,
}

