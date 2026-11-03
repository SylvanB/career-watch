//! Seed sources for company discovery.
//!
//! Each source implements [`SeedSource`] and returns a list of [`CompanyLead`]
//! items to be enqueued for probing.

use anyhow::Result;
use reqwest::Client;
use scraper::{Html, Selector};
use serde::Deserialize;
use tracing::warn;

/// A candidate company to probe, as returned by a seed source.
#[derive(Debug, Clone)]
pub struct CompanyLead {
    pub name: String,
    /// Bare hostname without scheme or trailing slash, e.g. `"stripe.com"`.
    pub domain: String,
    /// Human-readable label for where this lead came from.
    pub source: String,
}

/// Extract a bare domain from any URL string.
pub fn extract_domain(url: &str) -> Option<String> {
    let url = if url.starts_with("http") {
        url.to_string()
    } else {
        format!("https://{}", url)
    };

    ::url::Url::parse(&url)
        .ok()
        .and_then(|u| {
            u.host_str()
                .map(|h| h.trim_start_matches("www.").to_string())
        })
        .filter(|d| !d.is_empty() && d.contains('.'))
}

// ── Trait ────────────────────────────────────────────────────────────────────

/// A pluggable seed source that yields company leads.
#[async_trait::async_trait]
pub trait SeedSource: Send + Sync {
    async fn fetch(&self, client: &Client) -> Result<Vec<CompanyLead>>;
    fn name(&self) -> &str;
}

// ── Static list ──────────────────────────────────────────────────────────────

/// A hardcoded list of known companies.
pub struct StaticListSource {
    pub companies: Vec<CompanyLead>,
}

#[async_trait::async_trait]
impl SeedSource for StaticListSource {
    fn name(&self) -> &str {
        "static_list"
    }

    async fn fetch(&self, _client: &Client) -> Result<Vec<CompanyLead>> {
        Ok(self.companies.clone())
    }
}

// ── GitHub organisation ───────────────────────────────────────────────────────

/// Looks up a GitHub organisation by name and extracts its website as a lead.
pub struct GithubOrgSource {
    pub org: String,
}

#[derive(Deserialize)]
struct GithubOrgResponse {
    name: Option<String>,
    blog: Option<String>,
    company: Option<String>,
}

#[async_trait::async_trait]
impl SeedSource for GithubOrgSource {
    fn name(&self) -> &str {
        &self.org
    }

    async fn fetch(&self, client: &Client) -> Result<Vec<CompanyLead>> {
        let url = format!("https://api.github.com/orgs/{}", self.org);

        let resp: GithubOrgResponse = client
            .get(&url)
            .header("Accept", "application/vnd.github.v3+json")
            .header("User-Agent", "CareerWatch/1.0")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let blog = resp.blog.unwrap_or_default();
        let name = resp
            .name
            .or(resp.company)
            .unwrap_or_else(|| self.org.clone());

        if let Some(domain) = extract_domain(&blog) {
            return Ok(vec![CompanyLead {
                name,
                domain,
                source: format!("github_org:{}", self.org),
            }]);
        }

        Ok(vec![])
    }
}

// ── HTML startup directory ────────────────────────────────────────────────────

/// Scrapes an HTML startup-directory page for linked company domains.
///
/// `company_link_selector` should match `<a>` elements whose `href` points to
/// a company website. `name_selector` defaults to the text content of the
/// matched element if not provided.
pub struct StartupDirectorySource {
    pub url: String,
    /// CSS selector for `<a>` elements that link to company websites.
    pub company_link_selector: String,
}

#[async_trait::async_trait]
impl SeedSource for StartupDirectorySource {
    fn name(&self) -> &str {
        &self.url
    }

    async fn fetch(&self, client: &Client) -> Result<Vec<CompanyLead>> {
        let html = client
            .get(&self.url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        let document = Html::parse_document(&html);
        let selector = Selector::parse(&self.company_link_selector)
            .map_err(|e| anyhow::anyhow!("invalid CSS selector: {:?}", e))?;

        let mut leads = Vec::new();

        for el in document.select(&selector) {
            let href = el.value().attr("href").unwrap_or("").trim();
            let name = el.text().collect::<String>().trim().to_string();

            if href.is_empty() {
                continue;
            }

            match extract_domain(href) {
                Some(domain) => leads.push(CompanyLead {
                    name,
                    domain,
                    source: self.url.clone(),
                }),
                None => {
                    warn!(href, "could not extract domain from link");
                }
            }
        }

        Ok(leads)
    }
}

// ── GitHub org list ───────────────────────────────────────────────────────────

/// Fetches all public members of a GitHub organisation and collects their
/// profile website fields as company leads.
///
/// Useful when an org is an open-source foundation or developer collective
/// whose members are employed at different companies.
pub struct GithubOrgMembersSource {
    pub org: String,
    /// Optional GitHub personal access token for higher rate limits.
    pub token: Option<String>,
}

#[derive(Deserialize)]
struct GithubMember {
    login: String,
}

#[derive(Deserialize)]
struct GithubUser {
    name: Option<String>,
    company: Option<String>,
    blog: Option<String>,
}

#[async_trait::async_trait]
impl SeedSource for GithubOrgMembersSource {
    fn name(&self) -> &str {
        &self.org
    }

    async fn fetch(&self, client: &Client) -> Result<Vec<CompanyLead>> {
        let members_url = format!(
            "https://api.github.com/orgs/{}/members?per_page=100",
            self.org
        );

        let mut req = client
            .get(&members_url)
            .header("Accept", "application/vnd.github.v3+json")
            .header("User-Agent", "CareerWatch/1.0");

        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }

        let members: Vec<GithubMember> = req.send().await?.error_for_status()?.json().await?;

        let mut leads = Vec::new();

        for member in members {
            let user_url = format!("https://api.github.com/users/{}", member.login);
            let mut ureq = client
                .get(&user_url)
                .header("Accept", "application/vnd.github.v3+json")
                .header("User-Agent", "CareerWatch/1.0");

            if let Some(token) = &self.token {
                ureq = ureq.bearer_auth(token);
            }

            let user: GithubUser = match ureq.send().await?.error_for_status()?.json().await {
                Ok(u) => u,
                Err(e) => {
                    warn!(login = member.login, error = %e, "failed to fetch user profile");
                    continue;
                }
            };

            let blog = user.blog.unwrap_or_default();
            let name = user
                .name
                .or(user.company)
                .unwrap_or_else(|| member.login.clone());

            if let Some(domain) = extract_domain(&blog) {
                leads.push(CompanyLead {
                    name,
                    domain,
                    source: format!("github_org_members:{}", self.org),
                });
            }
        }

        Ok(leads)
    }
}

#[cfg(test)]
mod tests {
    use super::extract_domain;

    #[test]
    fn extracts_domain_from_full_url() {
        assert_eq!(
            extract_domain("https://www.stripe.com/about"),
            Some("stripe.com".to_string())
        );
    }

    #[test]
    fn extracts_domain_from_bare_domain() {
        assert_eq!(
            extract_domain("acme.example.com"),
            Some("acme.example.com".to_string())
        );
    }

    #[test]
    fn rejects_empty_string() {
        assert!(extract_domain("").is_none());
    }

    #[test]
    fn strips_www() {
        assert_eq!(
            extract_domain("https://www.github.com"),
            Some("github.com".to_string())
        );
    }
}
