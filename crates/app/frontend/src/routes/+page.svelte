<script lang="ts">
  import { onMount } from 'svelte';
  import { getSettings } from '$lib/api/commands';
  import {
    onHealthStatus,
    onOneDriveWarning,
    onSettingsCorrupt
  } from '$lib/api/events';
  import type {
    HealthStatus,
    OneDriveWarning,
    Settings,
    SettingsCorruptWarning
  } from '$lib/api/types';

  // Phase 5e: backend が emit する 3 種の event を listen し、初期 settings を
  // command で取得して表示する。
  // - HealthStatus: 2 秒毎の定期。projector backlog / DB サイズ / level を表示。
  // - SettingsCorruptWarning / OneDriveWarning: 起動時 1 回 (best-effort)。
  let settings = $state<Settings | null>(null);
  let settingsCorrupt = $state<SettingsCorruptWarning | null>(null);
  let onedrive = $state<OneDriveWarning | null>(null);
  let health = $state<HealthStatus | null>(null);
  let loadError = $state<string | null>(null);

  // 1024 で 4 段に丸めて B/KB/MB/GB 表記。健全運用なら DB は MB オーダー。
  function formatBytes(n: number): string {
    if (n <= 0) return '—';
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    let i = 0;
    let v = n;
    while (v >= 1024 && i < units.length - 1) {
      v /= 1024;
      i++;
    }
    return `${v.toFixed(v >= 100 || i === 0 ? 0 : 1)} ${units[i]}`;
  }

  onMount(() => {
    let unlistenSettings: (() => void) | undefined;
    let unlistenOneDrive: (() => void) | undefined;
    let unlistenHealth: (() => void) | undefined;

    // 並列 attach。setup() 直後の警告 emit を取りこぼさないよう同期的に開始。
    onSettingsCorrupt((p) => {
      settingsCorrupt = p;
    }).then((u) => {
      unlistenSettings = u;
    });
    onOneDriveWarning((p) => {
      onedrive = p;
    }).then((u) => {
      unlistenOneDrive = u;
    });
    onHealthStatus((p) => {
      health = p;
    }).then((u) => {
      unlistenHealth = u;
    });

    // 初期 settings を取得 (失敗してもアプリは続行可能)。
    getSettings()
      .then((s) => {
        settings = s;
      })
      .catch((e: unknown) => {
        loadError = String(e);
      });

    return () => {
      unlistenSettings?.();
      unlistenOneDrive?.();
      unlistenHealth?.();
    };
  });
</script>

<main>
  <h1>VRCWatchDog</h1>
  <p class="subtitle">Phase 5e — log_watcher + projector + health emitter wired.</p>

  <section class="health" data-level={health?.level ?? 'unknown'}>
    <h2>Health</h2>
    {#if health}
      <div class="metrics">
        <div class="metric">
          <span class="label">Level</span>
          <span class="value level-{health.level}">{health.level}</span>
        </div>
        <div class="metric">
          <span class="label">Backlog</span>
          <span class="value">{health.backlogSize.toLocaleString()}</span>
        </div>
        <div class="metric">
          <span class="label">DB size</span>
          <span class="value">{formatBytes(health.dbSizeBytes)}</span>
        </div>
        <div class="metric">
          <span class="label">Lag (s)</span>
          <span class="value">{health.projectorLagSec}</span>
        </div>
        <div class="metric">
          <span class="label">Free disk</span>
          <span class="value">{formatBytes(health.freeDiskBytes)}</span>
        </div>
      </div>
    {:else}
      <p class="dim">backend からの最初の health-status を待っています…</p>
    {/if}
  </section>

  {#if settingsCorrupt}
    <section class="warn">
      <strong>設定ファイルが破損しています</strong>
      <p>バックアップ: <code>{settingsCorrupt.backupPath}</code></p>
      <p>理由: {settingsCorrupt.reason}</p>
    </section>
  {/if}

  {#if onedrive}
    <section class="warn">
      <strong>DB が同期下にあります ({onedrive.detectedIndicator})</strong>
      <p>パス: <code>{onedrive.dbPath}</code></p>
      <p>SQLite WAL の同期競合を避けるため、`%LOCALAPPDATA%` 配下への配置を推奨します。</p>
    </section>
  {/if}

  <section>
    <h2>Settings</h2>
    {#if loadError}
      <p class="err">取得失敗: {loadError}</p>
    {:else if settings}
      <pre>{JSON.stringify(settings, null, 2)}</pre>
    {:else}
      <p class="dim">読込中…</p>
    {/if}
  </section>
</main>

<style>
  :global(html),
  :global(body) {
    margin: 0;
    padding: 0;
    background: #0b0b0c;
    color: #e5e5e5;
    font-family:
      system-ui,
      -apple-system,
      'Segoe UI',
      sans-serif;
  }

  main {
    padding: 2rem;
    min-height: 100vh;
    max-width: 720px;
    margin: 0 auto;
  }

  h1 {
    margin: 0 0 0.25rem 0;
    font-size: 1.75rem;
    font-weight: 600;
  }

  h2 {
    margin: 1.5rem 0 0.5rem 0;
    font-size: 1.1rem;
    font-weight: 500;
    opacity: 0.85;
  }

  .subtitle {
    margin: 0 0 1.5rem 0;
    opacity: 0.6;
    font-size: 0.9rem;
  }

  .health {
    background: #131418;
    border: 1px solid #2a2c33;
    border-radius: 6px;
    padding: 0.75rem 1rem 1rem;
    margin: 0 0 1rem 0;
  }
  .health[data-level='warning'] {
    border-color: #cd9f3a;
  }
  .health[data-level='degraded'] {
    border-color: #ff6b6b;
  }

  .metrics {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(110px, 1fr));
    gap: 0.5rem 1rem;
    margin-top: 0.25rem;
  }
  .metric {
    display: flex;
    flex-direction: column;
  }
  .metric .label {
    font-size: 0.7rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    opacity: 0.55;
  }
  .metric .value {
    font-family: 'JetBrains Mono', 'Cascadia Code', Consolas, monospace;
    font-size: 1rem;
  }
  .level-healthy {
    color: #6ed28a;
  }
  .level-warning {
    color: #f1c40f;
  }
  .level-degraded {
    color: #ff6b6b;
  }

  .dim {
    opacity: 0.55;
    font-size: 0.9rem;
    margin: 0;
  }

  .warn {
    background: #2a1f0f;
    border: 1px solid #cd9f3a;
    border-radius: 4px;
    padding: 0.75rem 1rem;
    margin: 0 0 1rem 0;
  }

  .warn strong {
    color: #f1c40f;
  }

  .warn p {
    margin: 0.25rem 0 0 0;
    font-size: 0.9rem;
  }

  .err {
    color: #ff6b6b;
  }

  code,
  pre {
    background: #181a1d;
    border-radius: 3px;
    padding: 0.15rem 0.35rem;
    font-family: 'JetBrains Mono', 'Cascadia Code', Consolas, monospace;
    font-size: 0.85rem;
  }

  pre {
    padding: 0.75rem;
    overflow-x: auto;
  }
</style>
