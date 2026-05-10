// Backend (`crates/core/src/ipc/events.rs` / `commands.rs`) と手動同期する TS 型。
//
// serde の `rename_all = "camelCase"` を一致させてある (events 側)。Settings は core 側で
// `rename_all = "snake_case"` なのでこちらも snake_case のまま。
//
// 将来的には ts-rs / specta などで自動生成に置き換えるが、Phase 5d では手書きで足りる。

export type HealthLevel = 'healthy' | 'warning' | 'degraded';

export interface HealthStatus {
  backlogSize: number;
  projectorLagSec: number;
  dbSizeBytes: number;
  freeDiskBytes: number;
  level: HealthLevel;
}

export interface SettingsCorruptWarning {
  backupPath: string;
  reason: string;
}

export interface OneDriveWarning {
  dbPath: string;
  detectedIndicator: string;
}

export interface FatalCorruptionEvent {
  rawEventId: number;
  reason: string;
}

export interface IngestProgress {
  processedLogFiles: number;
  totalRawEvents: number;
  /** ISO8601 UTC string. null when no events have been ingested yet. */
  lastEventAtUtc: string | null;
}

export interface UnknownLogFormatWarning {
  vrchatBuild: string | null;
  unparsableRatio: number;
}

// Settings は core::settings::store::Settings と一致させる (snake_case)。
export interface Settings {
  log_directory: string | null;
  photo_directory: string | null;
  thumbnail_cache_dir: string | null;
  locale: string;
  autostart_enabled: boolean;
  theme: string;
  notification_enabled: boolean;
}
