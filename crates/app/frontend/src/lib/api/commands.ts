// 型付き invoke() ラッパー。各 command 名は backend `crates/app/src/commands.rs` の
// `#[tauri::command] pub fn` の関数名と一致させる (Tauri は関数名を JS 側 invoke 名に使う)。

import { invoke } from '@tauri-apps/api/core';

import type {
  InitialWarnings,
  Notification,
  PhotoRecord,
  PlayerSession,
  RealtimeState,
  SelfPlayer,
  Settings,
  ThumbProgress,
  Video,
  Visit
} from './types';

/**
 * 写真原本を OS の関連付けされたアプリケーションで開く。
 *
 * Backend で path traversal validation (extension / canonicalize / scope) を通った
 * path のみが OS シェルに渡る。違反時は string error が reject される。
 */
export function openPhoto(filePath: string): Promise<void> {
  return invoke('open_photo', { filePath });
}

/** 写真の親 directory を Explorer で開く。 */
export function openPhotoFolder(filePath: string): Promise<void> {
  return invoke('open_photo_folder', { filePath });
}

/** 現在の設定 snapshot を取得。 */
export function getSettings(): Promise<Settings> {
  return invoke('get_settings');
}

/**
 * 設定を atomic に保存。SettingsWriter actor 経由で逐列化される。
 *
 * UI 側で連打されても順次処理 + 失敗時はエラーメッセージが reject される。
 */
export function saveSettings(settings: Settings): Promise<void> {
  return invoke('save_settings', { settings });
}

/**
 * 起動時に Bootstrap が検出した警告 (settings corrupt / DB OneDrive sync) を取得。
 *
 * setup() で event を emit すると onMount 前に取りこぼされるため、frontend が
 * onMount 直後にこの command を pull する方式にしてある。起動後に新規発生する
 * 警告は引き続き event 経由 (例: 設定変更で再 corrupt 検出時)。
 */
export function getInitialWarnings(): Promise<InitialWarnings> {
  return invoke('get_initial_warnings');
}

/**
 * 直近 `limit` 件の写真を `takenUtc` 降順で取得 (photo_grid 用)。
 *
 * limit <= 0 は backend 側で空配列に短絡される。Phase 6.3 で thumbSha が埋まれば
 * grid 表示で使う thumbnail URL を frontend で組み立てる予定。
 */
export function listRecentPhotos(limit: number): Promise<PhotoRecord[]> {
  return invoke('list_recent_photos', { limit });
}

/** 直近 `limit` 件の visit を `joinedUtc` 降順 + 紐づく写真数付きで取得。 */
export function listRecentVisits(limit: number): Promise<Visit[]> {
  return invoke('list_recent_visits', { limit });
}

/**
 * 指定 visit に紐づく写真を `takenUtc` 降順で最大 `limit` 件取得。
 * /history で visit を展開したときに inline でサムネ表示するために使う。
 */
export function listPhotosForVisit(visitId: number, limit: number): Promise<PhotoRecord[]> {
  return invoke('list_photos_for_visit', { visitId, limit });
}

/**
 * 指定 visit に居た player_sessions を `joinedUtc` 昇順で最大 `limit` 件取得。
 * /history で visit を展開したときに co-player 一覧を表示するために使う。
 */
export function listPlayersForVisit(visitId: number, limit: number): Promise<PlayerSession[]> {
  return invoke('list_players_for_visit', { visitId, limit });
}

/** 直近 `limit` 件の通知を `receivedUtc` 降順で取得。 */
export function listRecentNotifications(limit: number): Promise<Notification[]> {
  return invoke('list_recent_notifications', { limit });
}

/** 直近 `limit` 件の動画 URL を `detectedUtc` 降順で取得。 */
export function listRecentVideos(limit: number): Promise<Video[]> {
  return invoke('list_recent_videos', { limit });
}

/**
 * 直近の `User Authenticated` ログから抽出した「現在の自分」を取得。
 * VRChat に未ログインなら displayName=null。layout 等で表示するために使う。
 */
export function getSelfPlayer(): Promise<SelfPlayer> {
  return invoke('get_self_player');
}

/**
 * /realtime ページが mount 時に呼んで現状を seed する。
 * app 起動時に既に VRChat 動作中だった場合に、catch-up 中の LiveLogEvent を
 * 取りこぼした分を DB スナップショットから復元する。
 */
export function getRealtimeState(): Promise<RealtimeState> {
  return invoke('get_realtime_state');
}

/** thumb_writer の進捗 (ready / pending / total)。layout で 3s ごとに polling。 */
export function getThumbProgress(): Promise<ThumbProgress> {
  return invoke('get_thumb_progress');
}
