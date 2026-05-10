// `crates/core/src/ipc/events.rs::names` と完全一致させる。Phase 5d 時点では文字列定数の
// 二重メンテだが、テストもないので backend 側変更時に手で揃える。
//
// すべて `vrcwatchdog://` プレフィックス。

export const EVENT_HEALTH_STATUS = 'vrcwatchdog://health-status';
export const EVENT_SETTINGS_CORRUPT = 'vrcwatchdog://settings-corrupt';
export const EVENT_ONEDRIVE_WARNING = 'vrcwatchdog://onedrive-warning';
export const EVENT_FATAL_CORRUPTION = 'vrcwatchdog://fatal-corruption';
export const EVENT_INGEST_PROGRESS = 'vrcwatchdog://ingest-progress';
export const EVENT_UNKNOWN_LOG_FORMAT = 'vrcwatchdog://unknown-log-format';
