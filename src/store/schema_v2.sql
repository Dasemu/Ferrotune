-- v2: raw responses of Spotify's internal endpoints (spclient/pathfinder).
-- Keys are "<METHOD>:<host>:<path>#<body digest>"; never contain tokens.
CREATE TABLE IF NOT EXISTS http_cache (
    key        TEXT    PRIMARY KEY,
    body       BLOB    NOT NULL,
    fetched_at INTEGER NOT NULL   -- unix seconds
);
CREATE INDEX IF NOT EXISTS http_cache_fetched ON http_cache(fetched_at);
