<script lang="ts">
  import { onMount } from 'svelte';
  import { getSettings } from '$lib/api/commands';
  import { onOneDriveWarning, onSettingsCorrupt } from '$lib/api/events';
  import type { OneDriveWarning, Settings, SettingsCorruptWarning } from '$lib/api/types';

  // Phase 5d: backend が起動時に emit する 2 種の警告を listen し、初期 settings を
  // command で取得して表示する。リアルタイム画面 (health-status) は projector 連続
  // 実行とセットなので Phase 5e 以降。
  let settings = $state<Settings | null>(null);
  let settingsCorrupt = $state<SettingsCorruptWarning | null>(null);
  let onedrive = $state<OneDriveWarning | null>(null);
  let loadError = $state<string | null>(null);

  onMount(() => {
    let unlistenSettings: (() => void) | undefined;
    let unlistenOneDrive: (() => void) | undefined;

    // 並列に listener を貼る。setup() 直後に emit される warning を取りこぼさないよう
    // できるだけ早く購読する。
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

    // 初期 settings を取得。失敗してもアプリは続行可能。
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
    };
  });
</script>

<main>
  <h1>VRCWatchDog</h1>
  <p class="subtitle">Phase 5d — Tauri shell + bootstrap wired.</p>

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
      <p>読込中…</p>
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
