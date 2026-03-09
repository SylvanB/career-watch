#!/usr/bin/env bash
# Drop and recreate the local career_watch database, then run all migrations.
# Expects Postgres to be running via docker compose.

set -euo pipefail

DB_USER="${POSTGRES_USER:-career_watch}"
DB_PASS="${POSTGRES_PASSWORD:-password}"
DB_NAME="${POSTGRES_DB:-career_watch}"
DB_HOST="${POSTGRES_HOST:-localhost}"
DB_PORT="${POSTGRES_PORT:-5432}"
DATABASE_URL="postgres://${DB_USER}:${DB_PASS}@${DB_HOST}:${DB_PORT}/${DB_NAME}"

PSQL="docker compose exec -T postgres psql -U ${DB_USER}"

echo "→ Dropping database '${DB_NAME}' (if it exists)..."
$PSQL -d postgres -c "DROP DATABASE IF EXISTS ${DB_NAME};" 2>/dev/null || true

echo "→ Creating database '${DB_NAME}'..."
$PSQL -d postgres -c "CREATE DATABASE ${DB_NAME};"

echo "→ Running migrations..."
cargo sqlx migrate run --database-url "${DATABASE_URL}"

echo "✓ Done — ${DB_NAME} is fresh and up to date."
