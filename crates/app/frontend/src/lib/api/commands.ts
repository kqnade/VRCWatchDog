// 型付き invoke() ラッパー。各 command 名は backend `crates/app/src/commands.rs` の
// `#[tauri::command] pub fn` の関数名と一致させる (Tauri は関数名を JS 側 invoke 名に使う)。

import { invoke } from '@tauri-apps/api/core';

import type { Settings } from './types';

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
