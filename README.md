## Career Watch

**Career Watch** is a small SaaS-style service that lets users watch company career pages for changes and receive email notifications when something changes.

It is optimized for:
- **Career / jobs pages** that change infrequently.
- **Simple email notifications** instead of a full-blown dashboard.

The high-level design (from `OVERVIEW.md`):

- **Reverse proxy**: `caddy`
- **App server**: Rust binary (`career-watch-api`)
- **Database**: PostgreSQL
- **Queue**: Redis (optional – current implementation runs on cron without a queue)
- **Worker**: Rust binary (`career-watch-worker`) invoked by cron

```text
internet
   ↓
caddy
   ↓
rust app (career-watch-api)
   ↓
postgres

cron → rust worker (career-watch-worker)
```

---

## Project layout

- `src/main.rs` – HTTP API server entrypoint (`career-watch-api`).
- `src/bin/worker.rs` – scraping + email worker (`career-watch-worker`), designed to be run from cron.
- `src/lib.rs` – shared library (`AppState`, modules).
- `src/routes/` – HTTP routes (`/healthz`, `/api/watches`).
- `src/models.rs` – DB and API models for watches.
- `src/scraper.rs` – HTTP client + page hashing.
- `src/email.rs` – SMTP email sending using `lettre`.
- `src/config.rs` – environment-driven configuration.
- `migrations/` – `sqlx` migrations (PostgreSQL schema).
- `Caddyfile` – example Caddy reverse proxy configuration.
- `.env.example` – example environment configuration.

---

## Prerequisites

- **Rust**: stable toolchain via `rustup` (recent stable recommended).
- **PostgreSQL**: running instance reachable via `DATABASE_URL`.
- **Caddy** (optional but recommended) for local reverse proxy / TLS in production.
- **SMTP account**: e.g., Mailgun, SendGrid, or any SMTP server.

Optional:
- **Cron** on your host (for the worker).
- **Redis** if you later introduce queueing (not required by current code).

---

## Configuration

Configuration is read from environment variables via `AppConfig::from_env()` (see `src/config.rs`).

Copy the example file and adjust values:

```bash
cp .env.example .env
```

Required keys:

- `DATABASE_URL` – e.g. `postgres://career_watch:password@localhost:5432/career_watch`
- `BIND_ADDR` – bind address for the API server, default `0.0.0.0:8080`
- `SMTP_HOST` – SMTP server host
- `SMTP_PORT` – SMTP port, usually `587`
- `SMTP_USER` – SMTP username / API key
- `SMTP_PASS` – SMTP password / secret
- `SMTP_FROM` – from address, e.g. `Career Watch <no-reply@example.com>`
- `RUST_LOG` – optional, e.g. `info,tower_http=info`

---

## Database

Create a database and user, for example:

```sql
CREATE DATABASE career_watch;
CREATE USER career_watch WITH PASSWORD 'password';
GRANT ALL PRIVILEGES ON DATABASE career_watch TO career_watch;
```

Set `DATABASE_URL` accordingly, then migrations are applied automatically on startup via `sqlx::migrate!()`.

Schema (see `migrations/0001_init.sql`):

- `watchers` table:
  - `id UUID PRIMARY KEY`
  - `company_name TEXT`
  - `url TEXT`
  - `email TEXT`
  - `last_hash TEXT`
  - `last_checked_at TIMESTAMPTZ`
  - `created_at TIMESTAMPTZ DEFAULT now()`
  - `active BOOLEAN DEFAULT TRUE`

---

## Running the API server

From the project root, with `.env` configured:

```bash
cargo run --bin career-watch-api
```

This will:

- Load configuration from `.env`.
- Connect to Postgres and run migrations.
- Start the HTTP server on `BIND_ADDR` (by default `0.0.0.0:8080`).

Health check:

- `GET /healthz` → `200 OK`

Watch management API (JSON):

- `GET /api/watches` – list active watches.
- `POST /api/watches` – create a watch:

  ```json
  {
    "company_name": "Acme Corp",
    "url": "https://acme.example.com/careers",
    "email": "user@example.com"
  }
  ```

- `DELETE /api/watches/:id` – soft-delete a watch (`active = false`).

This is intentionally minimal; authorization / authn is out of scope for the base implementation.

---

## Running the worker

The worker scans all active watchers, fetches the target page, hashes the HTML body, and sends an email if the hash has changed since the last run.

Run once manually:

```bash
cargo run --bin career-watch-worker
```

Typical cron entry to run every 5 minutes (adjust paths):

```cron
*/5 * * * * cd /path/to/career-watch && cargo run --release --bin career-watch-worker >> /var/log/career-watch-worker.log 2>&1
```

Behavior:

- On first run for a watcher (no `last_hash`): store hash and timestamp, **no email**.
- On subsequent runs:
  - If hash changes: send email, update `last_hash` and `last_checked_at`.
  - If hash is unchanged: only update `last_checked_at`.

Retries / backoff are handled naively (log-and-continue); for serious production use you’d likely introduce a queue and better error handling.

---

## Caddy reverse proxy

An example `Caddyfile` is provided for local development:

```Caddyfile
career-watch.localhost {
    reverse_proxy /api/* 127.0.0.1:8080

    @static {
        not path /api/*
    }

    handle @static {
        root * ./public
        file_server
    }
}
```

From the project root:

```bash
caddy run
```

Then access the API at `http://career-watch.localhost/api/...`.

---

## Testing

Run all tests:

```bash
cargo test
```

Current tests cover:

- `AppConfig` env parsing and defaults.
- Page hashing behavior (`hash_body`).
- A router smoke-test to ensure the API router can be constructed.

---

## Notes / future work

Some obvious next steps if you want to grow this into a real SaaS:

- Add authentication and per-user ownership of watches.
- Add a small web UI for listing and managing watches.
- Add structured logging and metrics (e.g., Prometheus).
- Introduce a real job queue (Redis, SQS, etc.) instead of relying solely on cron.

