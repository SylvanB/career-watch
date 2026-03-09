//! Configuration loading for Career Watch.
//!
//! Values are sourced from environment variables and validated on startup.

use std::env;

/// Top-level application configuration loaded from environment variables.
#[derive(Clone, Debug)]
pub struct AppConfig {
    pub database_url: String,
    pub bind_addr: String,
    // SMTP
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_user: String,
    pub smtp_pass: String,
    pub smtp_from: String,
    /// When `false`, connects with no TLS (plain SMTP). Use for local dev
    /// with Mailpit. Defaults to `true` for production SMTP providers.
    pub smtp_tls: bool,
    // Stripe
    pub stripe_secret_key: String,
    pub stripe_webhook_secret: String,
    pub stripe_price_pro: String,
    pub stripe_price_business: String,
    // App
    /// Public base URL used to build links in emails (e.g. `https://careerwatch.io`).
    pub app_base_url: String,
    /// Static bearer token for internal admin endpoints.
    pub admin_secret: String,
    /// Whether to set the `Secure` flag on session cookies.
    /// Set to `false` for local HTTP dev; always `true` in production.
    pub cookie_secure: bool,
    /// Ollama base URL for local LLM selector detection.
    /// Leave blank to disable LLM-assisted selector detection.
    pub ollama_host: String,
    /// Ollama model to use for selector detection.
    /// Recommended: qwen2.5:3b or jinaai/reader-lm-v2
    pub ollama_model: String,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, anyhow::Error> {
        let database_url = env::var("DATABASE_URL")
            .map_err(|_| anyhow::anyhow!("DATABASE_URL must be set"))?;

        let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());

        let smtp_host =
            env::var("SMTP_HOST").map_err(|_| anyhow::anyhow!("SMTP_HOST must be set"))?;
        let smtp_port: u16 = env::var("SMTP_PORT")
            .unwrap_or_else(|_| "587".to_string())
            .parse()
            .map_err(|_| anyhow::anyhow!("SMTP_PORT must be a valid u16"))?;
        let smtp_user = env::var("SMTP_USER").unwrap_or_default();
        let smtp_pass = env::var("SMTP_PASS").unwrap_or_default();
        let smtp_from =
            env::var("SMTP_FROM").map_err(|_| anyhow::anyhow!("SMTP_FROM must be set"))?;
        let smtp_tls = env::var("SMTP_TLS")
            .unwrap_or_else(|_| "true".to_string())
            .to_lowercase()
            != "false";

        let stripe_secret_key = env::var("STRIPE_SECRET_KEY").unwrap_or_default();
        let stripe_webhook_secret = env::var("STRIPE_WEBHOOK_SECRET").unwrap_or_default();
        let stripe_price_pro = env::var("STRIPE_PRICE_PRO").unwrap_or_default();
        let stripe_price_business = env::var("STRIPE_PRICE_BUSINESS").unwrap_or_default();

        let app_base_url = env::var("APP_BASE_URL")
            .unwrap_or_else(|_| "http://localhost:8080".to_string());
        let admin_secret =
            env::var("ADMIN_SECRET").unwrap_or_else(|_| "change-me-in-production".to_string());
        let cookie_secure = env::var("COOKIE_SECURE")
            .unwrap_or_else(|_| "false".to_string())
            .to_lowercase()
            != "false";

        let ollama_host = env::var("OLLAMA_HOST")
            .unwrap_or_else(|_| "http://localhost:11434".to_string());
        let ollama_model = env::var("OLLAMA_MODEL")
            .unwrap_or_else(|_| "qwen2.5:3b".to_string());

        Ok(Self {
            database_url,
            bind_addr,
            smtp_host,
            smtp_port,
            smtp_user,
            smtp_pass,
            smtp_from,
            smtp_tls,
            stripe_secret_key,
            stripe_webhook_secret,
            stripe_price_pro,
            stripe_price_business,
            app_base_url,
            admin_secret,
            cookie_secure,
            ollama_host,
            ollama_model,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::AppConfig;
    use std::env;
    use std::sync::{Mutex, OnceLock};

    static ENV_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_MUTEX.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    fn clear_keys() {
        for key in [
            "DATABASE_URL",
            "BIND_ADDR",
            "SMTP_HOST",
            "SMTP_PORT",
            "SMTP_USER",
            "SMTP_PASS",
            "SMTP_FROM",
            "SMTP_TLS",
            "STRIPE_SECRET_KEY",
            "STRIPE_WEBHOOK_SECRET",
            "STRIPE_PRICE_PRO",
            "STRIPE_PRICE_BUSINESS",
            "APP_BASE_URL",
            "ADMIN_SECRET",
            "COOKIE_SECURE",
        ] {
            env::remove_var(key);
        }
    }

    #[test]
    fn loads_all_required_keys() {
        let _guard = env_lock();
        clear_keys();

        env::set_var("DATABASE_URL", "postgres://user:pass@localhost/db");
        env::set_var("BIND_ADDR", "127.0.0.1:9000");
        env::set_var("SMTP_HOST", "smtp.example.com");
        env::set_var("SMTP_PORT", "2525");
        env::set_var("SMTP_USER", "user");
        env::set_var("SMTP_PASS", "pass");
        env::set_var("SMTP_FROM", "no-reply@example.com");

        let cfg = AppConfig::from_env().expect("config should load");

        assert_eq!(cfg.database_url, "postgres://user:pass@localhost/db");
        assert_eq!(cfg.bind_addr, "127.0.0.1:9000");
        assert_eq!(cfg.smtp_host, "smtp.example.com");
        assert_eq!(cfg.smtp_port, 2525);

        clear_keys();
    }

    #[test]
    fn bind_addr_defaults_when_missing() {
        let _guard = env_lock();
        clear_keys();

        env::set_var("DATABASE_URL", "postgres://user:pass@localhost/db");
        env::set_var("SMTP_HOST", "smtp.example.com");
        env::set_var("SMTP_FROM", "no-reply@example.com");

        let cfg = AppConfig::from_env().expect("config should load");
        assert_eq!(cfg.bind_addr, "0.0.0.0:8080");

        clear_keys();
    }
}
