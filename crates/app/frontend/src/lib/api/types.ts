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

// `get_initial_warnings` command の戻り値。
// 起動時警告は event 経由だと onMount 前に取りこぼされるので、command で pull する。
export interface InitialWarnings {
  settingsCorrupt: SettingsCorruptWarning | null;
  dbSyncRisk: OneDriveWarning | null;
}

// `list_recent_photos` の戻り値要素。core::ipc::commands::PhotoRecordDto と一致させる。
//
// taken_naive_local は backend で `%Y-%m-%d %H:%M:%S` 文字列にしてあるので
// frontend は表示用に直接使える (Date 復元はしない)。
export interface PhotoRecord {
  id: number;
  filePath: string;
  fileName: string;
  takenNaiveLocal: string;
  /** ISO 8601 UTC string. */
  takenUtc: string;
  thumbSha: string | null;
  /** thumb_writer 完了後にのみ Some。`<thumb_dir>/<sha>.webp` の絶対パス。
   * convertFileSrc() で asset:// URL に変換して `<img>` に貼る。 */
  thumbPath: string | null;
  worldVisitId: number | null;
}

// `list_recent_visits` の戻り値要素。activity_history 画面で表示する。
//
// duration は backend で format_duration_hms に通した "HH:MM:SS" 文字列、
// あるいは ongoing visit (left_utc=null) なら "ongoing" マーカー。
export interface Visit {
  id: number;
  worldId: string | null;
  worldName: string;
  joinedUtc: string;
  leftUtc: string | null;
  resolutionState: string;
  photoCount: number;
  duration: string;
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
