-- self_player_records: User Authenticated イベント 1 件を 1 行で保持。
--
-- 同 source_raw_event_id で UNIQUE (idempotent re-projection)。
-- アカウント切替や再認証で複数 row 生まれるので、UI は authenticated_utc DESC で
-- 最新行を「現在の自分」として扱う。
CREATE TABLE self_player_records (
    id INTEGER PRIMARY KEY,
    source_raw_event_id INTEGER NOT NULL UNIQUE
        REFERENCES raw_log_events (id) ON DELETE CASCADE,
    display_name TEXT NOT NULL,
    authenticated_naive_local TEXT NOT NULL,
    authenticated_utc TEXT NOT NULL,
    authenticated_tz_id TEXT NOT NULL,
    authenticated_offset_seconds INTEGER NOT NULL,
    authenticated_resolution TEXT NOT NULL,
    authenticated_tz_source TEXT NOT NULL,
    authenticated_resolution_confidence TEXT NOT NULL
);
CREATE INDEX idx_self_player_authenticated_utc
    ON self_player_records (authenticated_utc DESC);
