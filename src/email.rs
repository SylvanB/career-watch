//! Email sending utilities for Career Watch.
//!
//! This module wraps `lettre` to send notification emails when
//! a watched career page changes. When job-level information is
//! available, emails include added/removed job postings.

use crate::config::AppConfig;
use crate::models::JobPosting;
use anyhow::Context;
use lettre::message::{header, Mailbox, Message};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

/// Asynchronous SMTP email client configured from application settings.
#[derive(Clone)]
pub struct EmailClient {
    mailer: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl EmailClient {
    /// Build an `EmailClient` from the provided [`AppConfig`].
    ///
    /// This validates SMTP host/port, credentials, and the `SMTP_FROM` address.
    pub fn from_config(config: &AppConfig) -> anyhow::Result<Self> {
        // Only attach credentials when they are actually provided — Mailpit
        // and other dev SMTP servers accept unauthenticated connections.
        let has_creds = !config.smtp_user.is_empty();
        let creds = Credentials::new(config.smtp_user.clone(), config.smtp_pass.clone());

        let mailer = if config.smtp_tls {
            // Production: STARTTLS on port 587 (or whatever SMTP_PORT is set to).
            let mut b = AsyncSmtpTransport::<Tokio1Executor>::relay(&config.smtp_host)
                .context("failed to configure SMTP relay")?
                .port(config.smtp_port);
            if has_creds {
                b = b.credentials(creds);
            }
            b.build()
        } else {
            // Local dev (Mailpit): plain SMTP, no TLS.
            let mut b = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&config.smtp_host)
                .port(config.smtp_port);
            if has_creds {
                b = b.credentials(creds);
            }
            b.build()
        };

        let from: Mailbox = config
            .smtp_from
            .parse()
            .context("invalid SMTP_FROM email address")?;

        Ok(Self { mailer, from })
    }

    /// Send a plain-text notification email describing a change to a watched
    /// career page.
    ///
    /// This is best-effort: failures are surfaced via `anyhow::Error` and the
    /// caller decides how to log or retry.
    pub async fn send_change_email(
        &self,
        to: &str,
        company_name: &str,
        url: &str,
        added: &[JobPosting],
        removed: &[JobPosting],
    ) -> anyhow::Result<()> {
        let to: Mailbox = to.parse().context("invalid recipient email")?;

        let subject = format!("Career page changed for {}", company_name);
        let mut body = String::new();

        if added.is_empty() && removed.is_empty() {
            body.push_str(&format!(
                "The watched career page for {company_name} has changed.\n\n"
            ));
        } else {
            body.push_str(&format!(
                "New changes detected for {company_name}:\n\n"
            ));

            if !added.is_empty() {
                body.push_str("New jobs:\n");
                for job in added {
                    body.push_str(" - ");
                    body.push_str(&job.title);
                    if let Some(ref posted) = job.posted_text {
                        body.push_str(" (");
                        body.push_str(posted);
                        body.push(')');
                    }
                    body.push('\n');
                }
                body.push('\n');
            }

            if !removed.is_empty() {
                body.push_str("Removed jobs:\n");
                for job in removed {
                    body.push_str(" - ");
                    body.push_str(&job.title);
                    body.push('\n');
                }
                body.push('\n');
            }
        }

        body.push_str(&format!("URL: {url}\n"));
        body.push_str("Visit the page to review the updates.\n");

        let email = Message::builder()
            .from(self.from.clone())
            .to(to)
            .header(header::ContentType::TEXT_PLAIN)
            .subject(subject)
            .body(body)
            .context("failed to build email message")?;

        self.mailer.send(email).await.context("failed to send email")?;

        Ok(())
    }
}

