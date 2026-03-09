-- Two-level selector support: card container + title/location children.
-- Replaces the fragile single-selector + case-boundary heuristic approach.
-- Populated by the LLM selector strategy during discovery.

ALTER TABLE companies
    ADD COLUMN IF NOT EXISTS title_selector    TEXT,
    ADD COLUMN IF NOT EXISTS location_selector TEXT;

ALTER TABLE watchers
    ADD COLUMN IF NOT EXISTS title_selector    TEXT,
    ADD COLUMN IF NOT EXISTS location_selector TEXT;
