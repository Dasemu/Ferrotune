-- Ferrotune local cache. Versioned with PRAGMA user_version.
-- The cache never contains credentials; it can be deleted at any time.

-- v1
CREATE TABLE IF NOT EXISTS entity (
    kind       TEXT    NOT NULL,   -- library | home | playlist | album | artist | liked | user
    id         TEXT    NOT NULL,
    json       TEXT    NOT NULL,   -- serde_json of the domain type
    updated_at INTEGER NOT NULL,   -- unix seconds
    PRIMARY KEY (kind, id)
);
CREATE INDEX IF NOT EXISTS entity_updated ON entity(updated_at);

CREATE TABLE IF NOT EXISTS kv (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
