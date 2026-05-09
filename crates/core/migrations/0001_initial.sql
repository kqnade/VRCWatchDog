-- VRCWatchDog initial schema (v6).
--
-- 設計の中核: ingest と projection の二系統 cursor、raw_log_events の永久保持、
-- projected_raw_events ledger による idempotency、resolution_state 5 状態機械。

CREATE TABLE processed_log_files (
    id INTEGER PRIMARY KEY,
    file_identity_hash TEXT NOT NULL UNIQUE,
    log_sequence_key TEXT NOT NULL,
    volume_serial INTEGER NOT NULL,
    file_id_high INTEGER NOT NULL,
    file_id_low INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    creation_time INTEGER NOT NULL,
    first_kb_hash TEXT NOT NULL,
    file_name TEXT NOT NULL,
    file_size INTEGER NOT NULL,
    mtime TEXT NOT NULL,
    ingest_position INTEGER NOT NULL DEFAULT 0,
    last_projected_raw_event_id INTEGER,
    tz_id TEXT NOT NULL,
    tz_source TEXT NOT NULL,
    processed_at TEXT NOT NULL
);
CREATE INDEX idx_pf_log_seq ON processed_log_files (log_sequence_key, generation);
CREATE INDEX idx_pf_logical
    ON processed_log_files (volume_serial, file_id_high, file_id_low, generation);

CREATE TABLE raw_log_events (
    id INTEGER PRIMARY KEY,
    processed_log_file_id INTEGER NOT NULL
        REFERENCES processed_log_files (id) ON DELETE RESTRICT,
    byte_offset INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    naive_local TEXT,
    UNIQUE (processed_log_file_id, byte_offset)
);
CREATE INDEX idx_rle_file_offset ON raw_log_events (processed_log_file_id, byte_offset);

CREATE TABLE projected_raw_events (
    raw_event_id INTEGER PRIMARY KEY
        REFERENCES raw_log_events (id) ON DELETE RESTRICT,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    projected_at TEXT,
    error TEXT
);
CREATE INDEX idx_pre_status ON projected_raw_events (status);

CREATE TABLE world_visits (
    id INTEGER PRIMARY KEY,
    source_raw_event_id INTEGER UNIQUE
        REFERENCES raw_log_events (id) ON DELETE RESTRICT,
    world_id TEXT,
    world_name TEXT NOT NULL,
    instance_id TEXT,
    resolution_state TEXT NOT NULL DEFAULT 'Pending',
    conflict_detail TEXT,
    joined_naive_local TEXT NOT NULL,
    joined_utc TEXT NOT NULL,
    joined_tz_id TEXT NOT NULL,
    joined_offset_seconds INTEGER NOT NULL,
    joined_resolution TEXT NOT NULL,
    joined_tz_source TEXT NOT NULL,
    joined_resolution_confidence TEXT NOT NULL,
    left_naive_local TEXT,
    left_utc TEXT,
    left_tz_id TEXT,
    left_offset_seconds INTEGER,
    left_resolution TEXT,
    left_tz_source TEXT,
    left_resolution_confidence TEXT
);
CREATE INDEX idx_wv_joined_utc ON world_visits (joined_utc);
CREATE INDEX idx_wv_world_id ON world_visits (world_id);
CREATE INDEX idx_wv_state ON world_visits (resolution_state);

CREATE TABLE player_sessions (
    id INTEGER PRIMARY KEY,
    source_raw_event_id INTEGER UNIQUE
        REFERENCES raw_log_events (id) ON DELETE RESTRICT,
    world_visit_id INTEGER NOT NULL
        REFERENCES world_visits (id) ON DELETE CASCADE,
    display_name TEXT NOT NULL,
    user_id TEXT,
    joined_naive_local TEXT NOT NULL,
    joined_utc TEXT NOT NULL,
    joined_tz_id TEXT NOT NULL,
    joined_offset_seconds INTEGER NOT NULL,
    joined_resolution TEXT NOT NULL,
    joined_tz_source TEXT NOT NULL,
    joined_resolution_confidence TEXT NOT NULL,
    left_naive_local TEXT,
    left_utc TEXT,
    left_tz_id TEXT,
    left_offset_seconds INTEGER,
    left_resolution TEXT,
    left_tz_source TEXT,
    left_resolution_confidence TEXT
);
CREATE INDEX idx_ps_display_name ON player_sessions (display_name);
CREATE INDEX idx_ps_user_id_joined ON player_sessions (user_id, joined_utc);

CREATE TABLE photo_records (
    id INTEGER PRIMARY KEY,
    file_path TEXT NOT NULL UNIQUE,
    file_name TEXT NOT NULL,
    taken_naive_local TEXT NOT NULL,
    taken_utc TEXT NOT NULL,
    taken_tz_id TEXT NOT NULL,
    taken_offset_seconds INTEGER NOT NULL,
    taken_resolution TEXT NOT NULL,
    taken_tz_source TEXT NOT NULL,
    taken_resolution_confidence TEXT NOT NULL,
    thumb_sha TEXT,
    world_visit_id INTEGER REFERENCES world_visits (id) ON DELETE SET NULL
);
CREATE INDEX idx_pr_taken_utc ON photo_records (taken_utc);

CREATE TABLE video_records (
    id INTEGER PRIMARY KEY,
    source_raw_event_id INTEGER UNIQUE
        REFERENCES raw_log_events (id) ON DELETE RESTRICT,
    url TEXT NOT NULL,
    title TEXT,
    thumbnail_url TEXT,
    thumbnail_sha TEXT,
    detected_naive_local TEXT NOT NULL,
    detected_utc TEXT NOT NULL,
    detected_tz_id TEXT NOT NULL,
    detected_offset_seconds INTEGER NOT NULL,
    detected_resolution TEXT NOT NULL,
    detected_tz_source TEXT NOT NULL,
    detected_resolution_confidence TEXT NOT NULL,
    world_visit_id INTEGER REFERENCES world_visits (id) ON DELETE SET NULL
);
CREATE INDEX idx_vr_detected_utc ON video_records (detected_utc);

CREATE TABLE notification_records (
    id INTEGER PRIMARY KEY,
    source_raw_event_id INTEGER UNIQUE
        REFERENCES raw_log_events (id) ON DELETE RESTRICT,
    received_naive_local TEXT NOT NULL,
    received_utc TEXT NOT NULL,
    received_tz_id TEXT NOT NULL,
    received_offset_seconds INTEGER NOT NULL,
    received_resolution TEXT NOT NULL,
    received_tz_source TEXT NOT NULL,
    received_resolution_confidence TEXT NOT NULL,
    sender_name TEXT NOT NULL,
    notification_type TEXT NOT NULL,
    world_visit_id INTEGER REFERENCES world_visits (id) ON DELETE SET NULL
);
CREATE INDEX idx_nr_received_utc ON notification_records (received_utc);

CREATE TABLE failed_video_lookups (
    normalized_key_sha TEXT PRIMARY KEY,
    raw_url TEXT NOT NULL,
    normalization_version TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    status TEXT NOT NULL,
    reason TEXT NOT NULL
);
