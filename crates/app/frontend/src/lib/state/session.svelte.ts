// Session-wide reactive state. layout が起動時 1 度だけ初期化し、各 page は読むだけ。
// これでページ遷移しても health 等が保持される ($state が module scope に居る)。

import type {
  HealthStatus,
  LiveLogEvent,
  OneDriveWarning,
  SelfPlayer,
  SettingsCorruptWarning,
  ThumbProgress,
} from '../api/types';

/** /realtime のログフィードを保持する上限件数。
 *  ページ遷移しても最後に届いた N 件は session に残しておく (currentWorld / presence は
 *  ページ mount で re-seed されるので、永続化するのはこの ring buffer だけ)。 */
export const REALTIME_LOG_MAX = 100;

/** session の event log 1 件。`seq` を {#each} の key に使うことで prepend ごとの
 *  全行 re-mount を避ける (位置 index を key に混ぜると毎 push で全 key が変わる)。 */
export type RealtimeLogEntry = { seq: number; event: LiveLogEvent };

class Session {
  health: HealthStatus | null = $state(null);
  settingsCorrupt: SettingsCorruptWarning | null = $state(null);
  onedrive: OneDriveWarning | null = $state(null);
  self: SelfPlayer | null = $state(null);
  thumbProgress: ThumbProgress | null = $state(null);

  /** /realtime のログ表示用 ring buffer。layout listener が push、ページが view。 */
  realtimeEventLog: RealtimeLogEntry[] = $state([]);
  /** /realtime の pause toggle。pause 中は layout listener が log buffer に追記しない。 */
  realtimePaused = $state(false);

  #seq = 0;

  pushRealtimeLog(ev: LiveLogEvent): void {
    if (this.realtimePaused) return;
    this.#seq += 1;
    this.realtimeEventLog = [
      { seq: this.#seq, event: ev },
      ...this.realtimeEventLog,
    ].slice(0, REALTIME_LOG_MAX);
  }

  clearRealtimeLog(): void {
    this.realtimeEventLog = [];
  }

  toggleRealtimePause(): void {
    this.realtimePaused = !this.realtimePaused;
  }
}

export const session = new Session();
