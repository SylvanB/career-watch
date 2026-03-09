## Career Watch

**Career Watch** monitors company career pages for job listing changes and sends email notifications when something changes. It also includes an automatic discovery pipeline that finds career pages from seed sources.

```text
internet
   ↓
Caddy (reverse proxy)
   ↓
career-watch-api  (Axum HTTP server)
   ↓
PostgreSQL

cron → career-watch-worker      (detect changes, send email)
cron → career-watch-discovery   (find new career pages)
```

---

## Quickstart (local dev)

### 1. Prerequisites

- [Rust](https://rustup.rs) stable toolchain
- [Docker](https://docs.docker.com/get-docker/) + Docker Compose

### 2. Start infrastructure

```sh
cp .env.example .env
make up
```

This starts:

| Service  | Address                    | Purpose                              |
|----------|----------------------------|--------------------------------------|
| Postgres | `localhost:5432`           | Database                             |
| Mailpit  | `http://localhost:8025`    | Local SMTP — catches all outbound email, nothing is delivered |

### 3. Run the API server

```sh
make run-api
# Migrations run automatically on startup.
# API available at http://localhost:8080
```

### 4. Run a worker pass

```sh
make run-worker       # check watchers for changes, send emails
make run-discovery    # discover new company career pages
```

### 5. Check emails

Open **http://localhost:8025** — every email the worker would send in production appears here.

---

## Local infrastructure (Docker Compose)

All infrastructure is defined in `docker-compose.yml`.

### Starting and stopping

```sh
make up       # start Postgres + Mailpit (background)
make down     # stop all containers
make logs     # tail container logs
```

### Postgres

| Setting   | Value          |
|-----------|----------------|
| Host      | `localhost`    |
| Port      | `5432`         |
| User      | `career_watch` |
| Password  | `password`     |
| Database  | `career_watch` |

```sh
make psql   # open a psql shell inside the container
```

### Mailpit (local SMTP)

Mailpit acts as a catch-all SMTP server. The `.env.example` is pre-configured to point at it:

```
SMTP_HOST=localhost
SMTP_PORT=1025
SMTP_USER=   (leave blank)
SMTP_PASS=   (leave blank)
```

Web UI: **http://localhost:8025**

### Running the app in Docker (optional)

The Rust binaries can also run inside Docker using the `app` Compose profile:

```sh
docker compose --profile app up
```

This builds and starts `api`, `worker`, and `discovery` containers. For most development work, running the binaries natively with `make run-*` is faster due to incremental compilation.

### Resetting the database

```sh
make db-reset
# Drops and recreates the career_watch database, then runs all migrations.
```

---

## Configuration

Copy the example and fill in values:

```sh
cp .env.example .env
```

| Variable       | Default (local dev)                                          | Description                              |
|----------------|--------------------------------------------------------------|------------------------------------------|
| `DATABASE_URL` | `postgres://career_watch:password@localhost:5432/career_watch` | PostgreSQL connection string           |
| `BIND_ADDR`    | `0.0.0.0:8080`                                               | API server bind address                  |
| `SMTP_HOST`    | `localhost`                                                  | SMTP server (use Mailpit locally)        |
| `SMTP_PORT`    | `1025`                                                       | SMTP port                                |
| `SMTP_USER`    | *(blank)*                                                    | SMTP username / API key                  |
| `SMTP_PASS`    | *(blank)*                                                    | SMTP password / secret                   |
| `SMTP_FROM`    | `Career Watch <no-reply@localhost>`                          | Sender address                           |
| `RUST_LOG`     | `info,tower_http=info`                                       | Log filter                               |

---

## API reference

### Health

```
GET /healthz  →  200 OK
```

### Watchers

| Method   | Path               | Description                   |
|----------|--------------------|-------------------------------|
| `GET`    | `/api/watches`     | List all active watchers      |
| `POST`   | `/api/watches`     | Create a watcher              |
| `DELETE` | `/api/watches/:id` | Soft-delete a watcher         |

**Create a watcher:**

```sh
curl -s -X POST http://localhost:8080/api/watches \
  -H 'Content-Type: application/json' \
  -d '{
    "company_name": "Stripe",
    "url": "https://stripe.com/jobs",
    "email": "you@example.com",
    "job_selector": "div.job-card"
  }'
```

`job_selector` is optional. When set, the worker extracts individual job postings using that CSS selector and diffs them run-to-run. When omitted, the worker falls back to full-page hash comparison.

---

## Workers

### career-watch-worker — change detection

Scans all active watchers, detects changes, and sends email notifications.

```sh
make run-worker
# or: cargo run --bin career-watch-worker
```

**Behaviour:**

- **First run** for a watcher: stores the baseline hash / job snapshot — no email sent.
- **Subsequent runs**: if the page changed (hash diff or job diff), an email is sent to the watcher's address.
- With `job_selector`: diffs added/removed job postings and includes them in the email.
- Without `job_selector`: reports that "the page changed" (hash-only mode).

**Cron example:**

```cron
*/15 * * * * cd /path/to/career-watch && ./career-watch-worker >> /var/log/career-watch-worker.log 2>&1
```

### career-watch-discovery — automatic discovery

Finds new company career pages from seed sources and stores them in the `companies` table. Periodically re-probes known companies.

```sh
make run-discovery
# or: cargo run --bin career-watch-discovery
```

**Pipeline:**

1. Fetch company leads from seed sources (static list, GitHub orgs, startup directories).
2. Insert new companies into `companies` (deduplicated by domain).
3. Probe each pending company — tries `/careers`, `/jobs`, `/join-us`, and 12 other common paths.
4. Detects the ATS platform (Greenhouse, Lever, Workday, Ashby, SmartRecruiters, etc.).
5. Validates that the page actually contains job listings.
6. Re-probes active companies whose `last_probed_at` is older than 24 hours.

**Cron example:**

```cron
0 */6 * * * cd /path/to/career-watch && ./career-watch-discovery >> /var/log/career-watch-discovery.log 2>&1
```

---

## Test server

A fake career page server for local end-to-end testing. It serves realistic HTML that the scraper, validator, and platform detector all understand. Job listings are held in memory and can be mutated at runtime via an admin API.

### Start

```sh
make run-test-server
# Career page:  http://localhost:9090/careers
# Admin UI:     http://localhost:9090/admin
```

### End-to-end walkthrough

```sh
# 1. Start infra + API (separate terminals)
make up
make run-api
make run-test-server

# 2. Create a watcher pointing at the test server
make test-watcher
# (uses TEST_EMAIL=test@localhost by default — override with TEST_EMAIL=you@example.com make test-watcher)

# 3. First worker run — establishes baseline, no email
make run-worker

# 4. Trigger a change
make test-add-job    # adds "Staff Engineer"
# or:
make test-rm-job     # removes "Senior Rust Engineer"

# 5. Second worker run — detects the change, sends email
make run-worker

# 6. View the email
open http://localhost:8025
```

### Admin API

| Method   | Path                     | Description                     |
|----------|--------------------------|---------------------------------|
| `GET`    | `/admin/jobs`            | List current jobs (JSON)        |
| `POST`   | `/admin/jobs`            | Add a job                       |
| `DELETE` | `/admin/jobs/:slug`      | Remove a job by slug            |
| `POST`   | `/admin/reset`           | Reset to default job listings   |

**Add a job:**

```sh
curl -s -X POST http://localhost:9090/admin/jobs \
  -H 'Content-Type: application/json' \
  -d '{
    "title": "Staff Engineer",
    "department": "Platform",
    "location": "Remote",
    "job_type": "Full-time"
  }'
```

**Remove a job** (slug = title lowercased, spaces replaced with `-`):

```sh
curl -s -X DELETE http://localhost:9090/admin/jobs/senior-rust-engineer
```

**Reset to defaults:**

```sh
curl -s -X POST http://localhost:9090/admin/reset
```

### HTML structure

The test server renders job cards that match the scraper's expectations exactly:

```html
<div class="job-card" data-job-id="senior-rust-engineer" data-posted="2025-01-01T00:00:00Z">
  <h3 class="job-title">Senior Rust Engineer</h3>
  <div class="job-meta">
    <span class="job-department">Engineering</span>
    <span class="job-location">Remote</span>
    <span class="job-type">Full-time</span>
  </div>
  <a href="/careers/senior-rust-engineer/apply" class="apply-link">Apply now</a>
</div>
```

Recommended `job_selector` when creating a watcher against the test server:

```
div.job-card h3.job-title
```

This targets only the title text, giving clean job names in the diff email. Using `div.job-card` instead would include department/location/type text in the title string.

---

## Database schema

Migrations live in `migrations/` and run automatically on startup.

### `watchers` — user-managed subscriptions

| Column           | Type          | Description                                      |
|------------------|---------------|--------------------------------------------------|
| `id`             | `UUID`        | Primary key                                      |
| `company_name`   | `TEXT`        | Display name                                     |
| `url`            | `TEXT`        | Career page URL to monitor                       |
| `email`          | `TEXT`        | Notification recipient                           |
| `last_hash`      | `TEXT`        | SHA-256 of last fetched page                     |
| `last_checked_at`| `TIMESTAMPTZ` | Timestamp of last successful check               |
| `created_at`     | `TIMESTAMPTZ` | Row creation time                                |
| `active`         | `BOOLEAN`     | Soft-delete flag                                 |
| `last_jobs_json` | `JSONB`       | Snapshot of last extracted job listings          |
| `job_selector`   | `TEXT`        | CSS selector for structured job extraction       |

### `companies` — auto-discovered companies

| Column          | Type          | Description                                       |
|-----------------|---------------|---------------------------------------------------|
| `id`            | `UUID`        | Primary key                                       |
| `name`          | `TEXT`        | Company name                                      |
| `domain`        | `TEXT UNIQUE` | Bare domain, e.g. `stripe.com`                    |
| `source`        | `TEXT`        | Where this lead came from                         |
| `career_url`    | `TEXT`        | Resolved career page URL after probing            |
| `platform`      | `TEXT`        | ATS platform: `greenhouse`, `lever`, `workday`, … |
| `platform_id`   | `TEXT`        | Company slug on the ATS platform                  |
| `status`        | `TEXT`        | `pending` → `active` or `failed`                  |
| `probe_error`   | `TEXT`        | Last error when `status = 'failed'`               |
| `created_at`    | `TIMESTAMPTZ` | Row creation time                                 |
| `last_probed_at`| `TIMESTAMPTZ` | Timestamp of last probe attempt                   |

---

## Project layout

```
src/
  main.rs                   API server entrypoint
  lib.rs                    Shared library (AppState, module exports)
  config.rs                 Environment config loading
  models.rs                 DB and API data models
  db.rs                     Database connection pool
  scraper.rs                HTTP fetching, SHA-256 hashing, CSS-based job extraction
  email.rs                  SMTP email client and notification templates
  routes/
    mod.rs                  Router setup
    health.rs               GET /healthz
    watches.rs              CRUD handlers for /api/watches
  discovery/
    mod.rs                  Pipeline overview and module exports
    platform.rs             ATS platform detection (URL + HTML signatures)
    validator.rs            Job listing page validation
    prober.rs               Career path prober (probes /careers, /jobs, etc.)
    sources.rs              Seed source trait + implementations
    scheduler.rs            Discovery queue, dedup, bounded-concurrency probing
  bin/
    worker.rs               career-watch-worker binary
    discovery_worker.rs     career-watch-discovery binary
    test_server.rs          career-watch-test-server binary

migrations/
  0001_init.sql             watchers table
  0002_jobs_snapshot.sql    last_jobs_json and job_selector columns
  0003_discovery.sql        companies table

scripts/
  db-reset.sh               Drop, recreate, and migrate the local database

docker-compose.yml          Postgres + Mailpit + optional app containers
Dockerfile                  Multi-stage build for all three binaries
Makefile                    Development task runner
Caddyfile                   Example Caddy reverse proxy config
.env.example                Environment variable template
```

---

## Caddy reverse proxy

An example `Caddyfile` is included for local development with a custom hostname:

```
career-watch.localhost {
    reverse_proxy /api/* 127.0.0.1:8080
}
```

```sh
caddy run
# API at http://career-watch.localhost/api/...
```

---

## Testing

```sh
make test
# or: cargo test
```

Tests cover config parsing, page hashing, job extraction, job diffing, platform detection, page validation, and domain extraction from URLs.

---

## Notes / future work

- Add authentication and per-user ownership of watches.
- Add a web UI for managing watches.
- Promote auto-discovered companies in `companies` to user-subscribed `watchers`.
- Add structured logging / Prometheus metrics.
- Replace cron with a persistent job queue (Redis, SQS, etc.) for more reliable scheduling.
