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
  /** world_visits.world_name を LEFT JOIN で取得した値。
   * worldVisitId が null なら必ず null。photo card に表示し、クリックで /history に遷移。 */
  worldName: string | null;
}

// `list_recent_videos` の戻り値要素。
// title / thumbnailUrl / thumbnailSha は video_info actor が noembed から
// fetch して埋める。thumbnailPath は backend が `<thumb_dir>/<sha>.webp` を組み立てる。
export interface Video {
  id: number;
  url: string;
  title: string | null;
  thumbnailUrl: string | null;
  thumbnailSha: string | null;
  /** thumbnail webp の絶対パス。convertFileSrc() で asset:// に変換して img 表示。 */
  thumbnailPath: string | null;
  detectedNaiveLocal: string;
  detectedUtc: string;
  worldVisitId: number | null;
}

// `list_recent_notifications` の戻り値要素。/notifications 画面で表示する。
export interface Notification {
  id: number;
  receivedNaiveLocal: string;
  receivedUtc: string;
  senderName: string;
  notificationType: string;
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
  /** 同 visit に居た player の unique 数 (user_id か display_name でユニーク化)。 */
  playerCount: number;
  duration: string;
}

// `vrcwatchdog://live-log-event` payload (Phase B)。
// core::ipc::events::LiveLogEvent と shape を一致させる (`tag = "kind"`)。
export type LiveLogEvent =
  | { kind: 'worldEntering'; naiveLocal: string; worldName: string }
  | {
      kind: 'worldJoining';
      naiveLocal: string;
      worldId: string;
      instanceId: string;
    }
  | {
      kind: 'playerJoined';
      naiveLocal: string;
      displayName: string;
      userId: string | null;
    }
  | {
      kind: 'playerLeft';
      naiveLocal: string;
      displayName: string;
      userId: string | null;
    }
  | {
      kind: 'notification';
      naiveLocal: string;
      sender: string;
      ntype: string;
    }
  | { kind: 'videoUrl'; naiveLocal: string; url: string };

// `get_self_player` の戻り値。1 度も VRChat にログインしていなければ
// displayName は null。
export interface SelfPlayer {
  displayName: string | null;
  authenticatedUtc: string | null;
}

// `list_players_for_visit` の戻り値要素。/history visit 詳細パネル用。
export interface PlayerSession {
  id: number;
  displayName: string;
  userId: string | null;
  joinedUtc: string;
  /** null なら visit 終了まで居続けたか、未終了の visit。 */
  leftUtc: string | null;
}

// Settings は core::settings::store::Settings と一致させる (snake_case)。
export interface Settings {
  log_directory: string | null;
  photo_directory: string | null;
  thumbnail_cache_dir: string | null;
  locale: string;
  autostart_enabled: boolean;
  /** "light" | "dark" | "system" */
  theme: string;
  /** "violet" | "blue" | "teal" | "green" | "amber" | "rose" | "slate" | "indigo" */
  accent_color: string;
  notification_enabled: boolean;
}
