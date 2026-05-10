// 型付き listen() ラッパー。Tauri の `@tauri-apps/api/event` を直接使うのと同じだが、
// event 名と payload 型のペアをここに集約しておくことで、各画面で文字列 typo や型
// ミスマッチを防ぐ。
//
// `listen()` は Promise<UnlistenFn> を返す。UnlistenFn を `onMount` の cleanup で
// 必ず呼び出すこと (svelte 側で `onMount(() => { let stop; listen(...).then(s => stop = s);
// return () => stop?.(); })` のパターン)。

import { listen, type UnlistenFn } from '@tauri-apps/api/event';

import {
  EVENT_FATAL_CORRUPTION,
  EVENT_HEALTH_STATUS,
  EVENT_INGEST_PROGRESS,
  EVENT_LIVE_LOG_EVENT,
  EVENT_ONEDRIVE_WARNING,
  EVENT_SETTINGS_CORRUPT,
  EVENT_UNKNOWN_LOG_FORMAT
} from './eventNames';
import type {
  FatalCorruptionEvent,
  HealthStatus,
  IngestProgress,
  LiveLogEvent,
  OneDriveWarning,
  SettingsCorruptWarning,
  UnknownLogFormatWarning
} from './types';

export function onHealthStatus(
  handler: (payload: HealthStatus) => void
): Promise<UnlistenFn> {
  return listen<HealthStatus>(EVENT_HEALTH_STATUS, (e) => handler(e.payload));
}

export function onSettingsCorrupt(
  handler: (payload: SettingsCorruptWarning) => void
): Promise<UnlistenFn> {
  return listen<SettingsCorruptWarning>(EVENT_SETTINGS_CORRUPT, (e) =>
    handler(e.payload)
  );
}

export function onOneDriveWarning(
  handler: (payload: OneDriveWarning) => void
): Promise<UnlistenFn> {
  return listen<OneDriveWarning>(EVENT_ONEDRIVE_WARNING, (e) => handler(e.payload));
}

export function onFatalCorruption(
  handler: (payload: FatalCorruptionEvent) => void
): Promise<UnlistenFn> {
  return listen<FatalCorruptionEvent>(EVENT_FATAL_CORRUPTION, (e) =>
    handler(e.payload)
  );
}

export function onIngestProgress(
  handler: (payload: IngestProgress) => void
): Promise<UnlistenFn> {
  return listen<IngestProgress>(EVENT_INGEST_PROGRESS, (e) => handler(e.payload));
}

export function onUnknownLogFormat(
  handler: (payload: UnknownLogFormatWarning) => void
): Promise<UnlistenFn> {
  return listen<UnknownLogFormatWarning>(EVENT_UNKNOWN_LOG_FORMAT, (e) =>
    handler(e.payload)
  );
}

/**
 * projector が Done に projection した直後の LiveLogEvent (Phase B)。
 * /realtime ページが ring buffer に積んでスクロール表示するために使う。
 * 各 batch ごとに複数 event が飛ぶことがある (catch-up 時など 100+)。
 */
export function onLiveLogEvent(
  handler: (payload: LiveLogEvent) => void
): Promise<UnlistenFn> {
  return listen<LiveLogEvent>(EVENT_LIVE_LOG_EVENT, (e) => handler(e.payload));
}
