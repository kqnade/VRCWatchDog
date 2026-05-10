// Session-wide reactive state. layout が起動時 1 度だけ初期化し、各 page は読むだけ。
// これでページ遷移しても health 等が保持される ($state が module scope に居る)。

import type {
  HealthStatus,
  OneDriveWarning,
  SelfPlayer,
  SettingsCorruptWarning,
  ThumbProgress,
} from '../api/types';

class Session {
  health: HealthStatus | null = $state(null);
  settingsCorrupt: SettingsCorruptWarning | null = $state(null);
  onedrive: OneDriveWarning | null = $state(null);
  self: SelfPlayer | null = $state(null);
  thumbProgress: ThumbProgress | null = $state(null);
}

export const session = new Session();
