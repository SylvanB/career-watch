//! Discovery pipeline for finding company career pages.
//!
//! ## Flow
//!
//! ```text
//! SeedSources
//!     │  fetch CompanyLeads
//!     ▼
//! DiscoveryScheduler::enqueue_companies()
//!     │  INSERT INTO companies (dedup by domain)
//!     ▼
//! companies WHERE status = 'pending'
//!     │
//!     ▼
//! CareerProber::probe_domain()
//!     │  GET https://{domain}/careers, /jobs, /join-us …
//!     ▼
//! detect_platform() + has_job_listings()
//!     │
//!     ▼
//! companies SET status = 'active', career_url = …
//!     │
//!     ▼
//! Re-probe on schedule (status = 'active', last_probed_at stale)
//! ```

pub mod platform;
pub mod prober;
pub mod scheduler;
pub mod selector;
pub mod sources;
pub mod validator;
