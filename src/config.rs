//! Configuration loading for Career Watch.
//!
//! Values are sourced from environment variables and validated on startup.

use std::env;

/// Top-level application configuration loaded from environment variables.
#[derive(Clone, Debug)]
pub struct AppConfig {
    pub database_url: String,
    pub bind_addr: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_user: String,
    pub smtp_pass: String,
    pub smtp_from: String,
}

impl AppConfig {
    /// Construct an `AppConfig` from environment variables.
    ///
    /// All SMTP and database-related keys are required. `BIND_ADDR` is optional
    /// and defaults to `0.0.0.0:8080` when omitted.
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
        let smtp_user =
            env::var("SMTP_USER").map_err(|_| anyhow::anyhow!("SMTP_USER must be set"))?;
        let smtp_pass =
            env::var("SMTP_PASS").map_err(|_| anyhow::anyhow!("SMTP_PASS must be set"))?;
        let smtp_from =
            env::var("SMTP_FROM").map_err(|_| anyhow::anyhow!("SMTP_FROM must be set"))?;

        Ok(Self {
            database_url,
            bind_addr,
            smtp_host,
            smtp_port,
            smtp_user,
            smtp_pass,
            smtp_from,
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
        assert_eq!(cfg.smtp_user, "user");
        assert_eq!(cfg.smtp_pass, "pass");
        assert_eq!(cfg.smtp_from, "no-reply@example.com");

        clear_keys();
    }

    #[test]
    fn bind_addr_defaults_when_missing() {
        let _guard = env_lock();
        clear_keys();

        env::set_var("DATABASE_URL", "postgres://user:pass@localhost/db");
        env::set_var("SMTP_HOST", "smtp.example.com");
        env::set_var("SMTP_PORT", "587");
        env::set_var("SMTP_USER", "user");
        env::set_var("SMTP_PASS", "pass");
        env::set_var("SMTP_FROM", "no-reply@example.com");

        let cfg = AppConfig::from_env().expect("config should load");
        assert_eq!(cfg.bind_addr, "0.0.0.0:8080");

        clear_keys();
    }
}

