// Phase F: 軽量 i18n。external lib (paraglide / svelte-i18n) を入れず、
// 静的型 + シンプルな store で済ませる。全 UI 文字列は `Messages` interface に
// 集約し、ja / en の 2 record を提供する。
//
// 1 string = 1 key 方針。ICU 風の placeholder が必要な場面は `{name}` のような
// プレースホルダにし、t(...) 呼び出し側で `.replace(/\{name\}/g, value)` する。

export type Locale = 'ja' | 'en';

export const SUPPORTED_LOCALES: readonly Locale[] = ['ja', 'en'] as const;

/** Tauri command 経由で受ける `settings.locale` 文字列を `Locale` に正規化。
 *  未対応値は ja に倒す (現状 OS 区別なし、UI 上は明示選択を前提)。 */
export function normalizeLocale(input: string | null | undefined): Locale {
  if (input === 'en' || input === 'ja') return input;
  return 'ja';
}

/** すべての翻訳キーを 1 箇所に集約した辞書型。新規キー追加時は ja/en 両方に書く。 */
export interface Messages {
  // global / nav
  appName: string;
  navHome: string;
  navRealtime: string;
  navPhotos: string;
  navHistory: string;
  navNotifications: string;
  navVideos: string;
  navSettings: string;
  navHomeBack: string;
  loading: string;
  // home
  homeSubtitle: string;
  healthHeading: string;
  healthLevel: string;
  healthBacklog: string;
  healthDbSize: string;
  healthLag: string;
  healthFreeDisk: string;
  healthWaiting: string;
  warnSettingsCorruptTitle: string;
  warnSettingsCorruptBackup: string;
  warnSettingsCorruptReason: string;
  warnOneDriveTitle: string;
  warnOneDrivePath: string;
  warnOneDriveAdvice: string;
  settingsHeading: string;
  settingsLoadFailed: string;
  // photos
  photosTitle: string;
  photosCountFormat: string; // "直近 {count} 件 (最大 {max} 件)"
  photosEmpty: string;
  photoOpenError: string;
  photoFolderOpenError: string;
  photoOpenHint: string;
  photoNoVisit: string;
  photoFolderBtn: string;
  photoNoThumb: string;
  // history
  historyTitle: string;
  historyEmpty: string;
  historyJoined: string;
  historyDuration: string;
  historyPhotos: string;
  historyPlayers: string;
  playersHeading: string; // "Players ({count})"
  playersEmpty: string;
  playersLoading: string;
  photosLoading: string;
  photosNoneInVisit: string;
  visitOpenInHistory: string; // tooltip
  // notifications
  notificationsTitle: string;
  notificationsEmpty: string;
  notificationsFilteredEmpty: string;
  notifViewTimeline: string;
  notifViewBySender: string;
  filterClear: string;
  senderLatest: string; // "{count} 件 · 最新 {time}"
  // videos
  videosTitle: string;
  videosEmpty: string;
  videoNoThumb: string;
  // realtime
  realtimeTitle: string;
  realtimeFeedHeading: string;
  realtimeWaiting: string;
  realtimeWorld: string;
  realtimeWorldUnknown: string;
  realtimePresence: string;
  realtimePresenceEmpty: string;
  realtimePauseBtn: string;
  realtimeResumeBtn: string;
  realtimeClearBtn: string;
  // settings
  settingsTitle: string;
  settingsLogDir: string;
  settingsPhotoDir: string;
  settingsThumbCache: string;
  settingsLocale: string;
  settingsTheme: string;
  settingsAutostart: string;
  settingsNotificationEnabled: string;
  settingsBrowse: string;
  settingsSave: string;
  settingsSaved: string;
  settingsSaveFailed: string;
  settingsDirty: string;
  settingsThemeDark: string;
  settingsThemeLight: string;
}

export const messages: Record<Locale, Messages> = {
  ja: {
    appName: 'VRCWatchDog',
    navHome: 'Home',
    navRealtime: 'リアルタイム',
    navPhotos: '写真',
    navHistory: '履歴',
    navNotifications: '通知',
    navVideos: '動画',
    navSettings: '設定',
    navHomeBack: '← ホーム',
    loading: '読込中…',
    homeSubtitle: 'log_watcher + projector + photo_scanner running.',
    healthHeading: 'ヘルス',
    healthLevel: 'レベル',
    healthBacklog: 'バックログ',
    healthDbSize: 'DB サイズ',
    healthLag: '遅延 (秒)',
    healthFreeDisk: '空き容量',
    healthWaiting: 'バックエンドからの初回 health-status を待っています…',
    warnSettingsCorruptTitle: '設定ファイルが破損しています',
    warnSettingsCorruptBackup: 'バックアップ:',
    warnSettingsCorruptReason: '理由: ',
    warnOneDriveTitle: 'DB が同期下にあります ({indicator})',
    warnOneDrivePath: 'パス:',
    warnOneDriveAdvice:
      'SQLite WAL の同期競合を避けるため、`%LOCALAPPDATA%` 配下への配置を推奨します。',
    settingsHeading: '設定',
    settingsLoadFailed: '取得失敗:',
    photosTitle: '写真',
    photosCountFormat: '直近 {count} 件 (最大 {max} 件)',
    photosEmpty:
      '写真がまだ取り込まれていません。Settings で photo_directory を設定してください。',
    photoOpenError: '写真を開けませんでした:',
    photoFolderOpenError: 'フォルダを開けませんでした:',
    photoOpenHint: 'クリック / Enter で開く',
    photoNoVisit: '紐づく visit 無し',
    photoFolderBtn: '📁 フォルダ',
    photoNoThumb: 'サムネ未生成',
    historyTitle: '履歴',
    historyEmpty: 'まだ visit がありません。VRChat の起動を待っています。',
    historyJoined: '入室',
    historyDuration: '滞在',
    historyPhotos: '写真',
    historyPlayers: 'プレイヤー',
    playersHeading: 'プレイヤー ({count})',
    playersEmpty: '同居プレイヤーの記録はありません',
    playersLoading: '読込中…',
    photosLoading: '読込中…',
    photosNoneInVisit: 'この visit に紐づく写真はありません',
    visitOpenInHistory: '紐づく visit を /history で開く',
    notificationsTitle: '通知',
    notificationsEmpty: '通知ログはまだ記録されていません。',
    notificationsFilteredEmpty: 'フィルタにマッチする通知はありません。',
    notifViewTimeline: '時系列',
    notifViewBySender: '送信者ごと',
    filterClear: 'クリア',
    senderLatest: '{count} 件 · 最新 {time}',
    videosTitle: '動画',
    videosEmpty: '動画ログはまだ記録されていません。',
    videoNoThumb: 'サムネ未取得',
    realtimeTitle: 'リアルタイム',
    realtimeFeedHeading: 'ログフィード',
    realtimeWaiting: 'バックエンドからの live event を待っています…',
    realtimeWorld: '現在のワールド',
    realtimeWorldUnknown: '未入室 (VRChat 起動を待機中)',
    realtimePresence: '同居プレイヤー',
    realtimePresenceEmpty: '同居プレイヤー無し',
    realtimePauseBtn: '一時停止',
    realtimeResumeBtn: '再開',
    realtimeClearBtn: 'クリア',
    settingsTitle: '設定',
    settingsLogDir: 'VRChat ログディレクトリ',
    settingsPhotoDir: '写真ディレクトリ',
    settingsThumbCache: 'サムネキャッシュディレクトリ',
    settingsLocale: '言語',
    settingsTheme: 'テーマ',
    settingsAutostart: 'OS 起動時に自動開始',
    settingsNotificationEnabled: '通知を表示',
    settingsBrowse: '参照…',
    settingsSave: '保存',
    settingsSaved: '保存しました',
    settingsSaveFailed: '保存失敗:',
    settingsDirty: '未保存の変更があります',
    settingsThemeDark: 'ダーク',
    settingsThemeLight: 'ライト',
  },
  en: {
    appName: 'VRCWatchDog',
    navHome: 'Home',
    navRealtime: 'Realtime',
    navPhotos: 'Photos',
    navHistory: 'History',
    navNotifications: 'Notifications',
    navVideos: 'Videos',
    navSettings: 'Settings',
    navHomeBack: '← Home',
    loading: 'Loading…',
    homeSubtitle: 'log_watcher + projector + photo_scanner running.',
    healthHeading: 'Health',
    healthLevel: 'Level',
    healthBacklog: 'Backlog',
    healthDbSize: 'DB size',
    healthLag: 'Lag (s)',
    healthFreeDisk: 'Free disk',
    healthWaiting: 'Waiting for first health-status from backend…',
    warnSettingsCorruptTitle: 'Settings file is corrupt',
    warnSettingsCorruptBackup: 'Backup:',
    warnSettingsCorruptReason: 'Reason:',
    warnOneDriveTitle: 'DB is on a sync-managed location ({indicator})',
    warnOneDrivePath: 'Path:',
    warnOneDriveAdvice:
      'To avoid SQLite WAL sync conflicts, prefer storing the DB under `%LOCALAPPDATA%`.',
    settingsHeading: 'Settings',
    settingsLoadFailed: 'Load failed:',
    photosTitle: 'Photos',
    photosCountFormat: '{count} most recent (limit {max})',
    photosEmpty:
      'No photos have been ingested yet. Set photo_directory in Settings.',
    photoOpenError: 'Could not open photo:',
    photoFolderOpenError: 'Could not open folder:',
    photoOpenHint: 'Click / Enter to open',
    photoNoVisit: 'no linked visit',
    photoFolderBtn: '📁 folder',
    photoNoThumb: 'no thumb yet',
    historyTitle: 'Activity History',
    historyEmpty: 'No visits yet. Waiting for VRChat to start.',
    historyJoined: 'Joined',
    historyDuration: 'Duration',
    historyPhotos: 'Photos',
    historyPlayers: 'Players',
    playersHeading: 'Players ({count})',
    playersEmpty: 'No co-player records.',
    playersLoading: 'Loading…',
    photosLoading: 'Loading…',
    photosNoneInVisit: 'No photos linked to this visit.',
    visitOpenInHistory: 'Open linked visit in /history',
    notificationsTitle: 'Notifications',
    notificationsEmpty: 'No notifications yet.',
    notificationsFilteredEmpty: 'No notifications match the filter.',
    notifViewTimeline: 'Timeline',
    notifViewBySender: 'By sender',
    filterClear: 'clear',
    senderLatest: '{count} entries · latest {time}',
    videosTitle: 'Videos',
    videosEmpty: 'No video URLs detected yet.',
    videoNoThumb: 'no thumb',
    realtimeTitle: 'Realtime',
    realtimeFeedHeading: 'Log Feed',
    realtimeWaiting: 'Waiting for live events from backend…',
    realtimeWorld: 'Current World',
    realtimeWorldUnknown: 'Not joined (waiting for VRChat to start)',
    realtimePresence: 'Players in instance',
    realtimePresenceEmpty: 'No co-players present',
    realtimePauseBtn: 'pause',
    realtimeResumeBtn: 'resume',
    realtimeClearBtn: 'clear',
    settingsTitle: 'Settings',
    settingsLogDir: 'VRChat log directory',
    settingsPhotoDir: 'Photo directory',
    settingsThumbCache: 'Thumbnail cache directory',
    settingsLocale: 'Language',
    settingsTheme: 'Theme',
    settingsAutostart: 'Launch on OS startup',
    settingsNotificationEnabled: 'Show notifications',
    settingsBrowse: 'Browse…',
    settingsSave: 'Save',
    settingsSaved: 'Saved',
    settingsSaveFailed: 'Save failed:',
    settingsDirty: 'Unsaved changes',
    settingsThemeDark: 'Dark',
    settingsThemeLight: 'Light',
  },
};
