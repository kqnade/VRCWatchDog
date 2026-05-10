<script lang="ts">
  import { onMount } from 'svelte';
  import { getInitialWarnings, getSettings } from '$lib/api/commands';
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

  // Phase 5e/5f:
  // - HealthStatus: 2 秒毎の定期 event。projector backlog / DB サイズ / level を表示。
  // - 起動時警告 (settings corrupt / OneDrive risk): onMount で `getInitialWarnings`
  //   を pull (event は listener attach 前に飛んで取りこぼされるため)。
  // - 起動後に新規発生する警告は引き続き event 経由 (再 corrupt 検出など)。
  let settings = $state<Settings | null>(null);
  let settingsCorrupt = $state<SettingsCorruptWarning | null>(null);
  let onedrive = $state<OneDriveWarning | null>(null);
  let health = $state<HealthStatus | null>(null);
  let loadError = $state<string | null>(null);

  // health の level に応じた border 色 class。Tailwind は data-* attribute selector を
  // 直接書きづらいので $derived + class binding で表現する。
  const healthBorderClass = $derived(
    health?.level === 'degraded'
      ? 'border-destructive'
      : health?.level === 'warning'
        ? 'border-warning'
        : 'border-border'
  );

  const levelTextClass = $derived(
    health?.level === 'degraded'
      ? 'text-destructive'
      : health?.level === 'warning'
        ? 'text-warning-foreground'
        : 'text-success'
  );

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

    getInitialWarnings()
      .then((w) => {
        if (w.settingsCorrupt) settingsCorrupt = w.settingsCorrupt;
        if (w.dbSyncRisk) onedrive = w.dbSyncRisk;
      })
      .catch((e: unknown) => {
        console.error('getInitialWarnings failed:', e);
      });

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

<main class="mx-auto min-h-screen max-w-3xl p-8">
  <h1 class="mb-1 text-2xl font-semibold">VRCWatchDog</h1>
  <p class="mb-6 text-sm opacity-60">
    Phase 5g — Tailwind v4 + semantic theme tokens.
  </p>

  <section class="mb-4 rounded-md border bg-card p-4 {healthBorderClass}">
    <h2 class="mb-2 text-lg font-medium opacity-85">Health</h2>
    {#if health}
      <div
        class="mt-1 grid gap-x-4 gap-y-2"
        style="grid-template-columns: repeat(auto-fit, minmax(110px, 1fr));"
      >
        <div class="flex flex-col">
          <span class="text-xs uppercase tracking-wider opacity-55">Level</span>
          <span class="font-mono text-base {levelTextClass}">{health.level}</span>
        </div>
        <div class="flex flex-col">
          <span class="text-xs uppercase tracking-wider opacity-55">Backlog</span>
          <span class="font-mono text-base">{health.backlogSize.toLocaleString()}</span>
        </div>
        <div class="flex flex-col">
          <span class="text-xs uppercase tracking-wider opacity-55">DB size</span>
          <span class="font-mono text-base">{formatBytes(health.dbSizeBytes)}</span>
        </div>
        <div class="flex flex-col">
          <span class="text-xs uppercase tracking-wider opacity-55">Lag (s)</span>
          <span class="font-mono text-base">{health.projectorLagSec}</span>
        </div>
        <div class="flex flex-col">
          <span class="text-xs uppercase tracking-wider opacity-55">Free disk</span>
          <span class="font-mono text-base">{formatBytes(health.freeDiskBytes)}</span>
        </div>
      </div>
    {:else}
      <p class="text-sm opacity-55">backend からの最初の health-status を待っています…</p>
    {/if}
  </section>

  {#if settingsCorrupt}
    <section
      class="mb-4 rounded border border-warning bg-[var(--color-warning-bg)] px-4 py-3"
    >
      <strong class="text-warning-foreground">設定ファイルが破損しています</strong>
      <p class="mt-1 text-sm">
        バックアップ:
        <code class="rounded bg-muted px-1.5 py-0.5 font-mono text-sm"
          >{settingsCorrupt.backupPath}</code
        >
      </p>
      <p class="mt-1 text-sm">理由: {settingsCorrupt.reason}</p>
    </section>
  {/if}

  {#if onedrive}
    <section
      class="mb-4 rounded border border-warning bg-[var(--color-warning-bg)] px-4 py-3"
    >
      <strong class="text-warning-foreground"
        >DB が同期下にあります ({onedrive.detectedIndicator})</strong
      >
      <p class="mt-1 text-sm">
        パス:
        <code class="rounded bg-muted px-1.5 py-0.5 font-mono text-sm">{onedrive.dbPath}</code>
      </p>
      <p class="mt-1 text-sm">
        SQLite WAL の同期競合を避けるため、`%LOCALAPPDATA%` 配下への配置を推奨します。
      </p>
    </section>
  {/if}

  <section>
    <h2 class="mb-2 mt-6 text-lg font-medium opacity-85">Settings</h2>
    {#if loadError}
      <p class="text-destructive">取得失敗: {loadError}</p>
    {:else if settings}
      <pre class="overflow-x-auto rounded bg-muted p-3 font-mono text-sm">{JSON.stringify(
          settings,
          null,
          2
        )}</pre>
    {:else}
      <p class="text-sm opacity-55">読込中…</p>
    {/if}
  </section>
</main>
