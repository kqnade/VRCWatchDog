<script lang="ts">
  import { onMount } from 'svelte';
  import { getInitialWarnings, getSettings } from '$lib/api/commands';
  import {
    onHealthStatus,
    onOneDriveWarning,
    onSettingsCorrupt
  } from '$lib/api/events';
  import { i18n } from '$lib/i18n/use_t.svelte';
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
  <h1 class="mb-1 text-2xl font-semibold">{i18n.t('appName')}</h1>
  <p class="mb-2 text-sm opacity-60">{i18n.t('homeSubtitle')}</p>
  <nav class="mb-6 flex flex-wrap gap-4 text-sm">
    <a href="/realtime" class="text-muted-foreground hover:underline">{i18n.t('navRealtime')} →</a>
    <a href="/photos" class="text-muted-foreground hover:underline">{i18n.t('navPhotos')} →</a>
    <a href="/history" class="text-muted-foreground hover:underline">{i18n.t('navHistory')} →</a>
    <a href="/notifications" class="text-muted-foreground hover:underline">{i18n.t('navNotifications')} →</a>
    <a href="/videos" class="text-muted-foreground hover:underline">{i18n.t('navVideos')} →</a>
    <a href="/settings" class="text-muted-foreground hover:underline">{i18n.t('navSettings')} →</a>
  </nav>

  <section class="mb-4 rounded-md border bg-card p-4 {healthBorderClass}">
    <h2 class="mb-2 text-lg font-medium opacity-85">{i18n.t('healthHeading')}</h2>
    {#if health}
      <div
        class="mt-1 grid gap-x-4 gap-y-2"
        style="grid-template-columns: repeat(auto-fit, minmax(110px, 1fr));"
      >
        <div class="flex flex-col">
          <span class="text-xs uppercase tracking-wider opacity-55">{i18n.t('healthLevel')}</span>
          <span class="font-mono text-base {levelTextClass}">{health.level}</span>
        </div>
        <div class="flex flex-col">
          <span class="text-xs uppercase tracking-wider opacity-55">{i18n.t('healthBacklog')}</span>
          <span class="font-mono text-base">{health.backlogSize.toLocaleString()}</span>
        </div>
        <div class="flex flex-col">
          <span class="text-xs uppercase tracking-wider opacity-55">{i18n.t('healthDbSize')}</span>
          <span class="font-mono text-base">{formatBytes(health.dbSizeBytes)}</span>
        </div>
        <div class="flex flex-col">
          <span class="text-xs uppercase tracking-wider opacity-55">{i18n.t('healthLag')}</span>
          <span class="font-mono text-base">{health.projectorLagSec}</span>
        </div>
        <div class="flex flex-col">
          <span class="text-xs uppercase tracking-wider opacity-55">{i18n.t('healthFreeDisk')}</span>
          <span class="font-mono text-base">{formatBytes(health.freeDiskBytes)}</span>
        </div>
      </div>
    {:else}
      <p class="text-sm opacity-55">{i18n.t('healthWaiting')}</p>
    {/if}
  </section>

  {#if settingsCorrupt}
    <section
      class="mb-4 rounded border border-warning bg-[var(--color-warning-bg)] px-4 py-3"
    >
      <strong class="text-warning-foreground">{i18n.t('warnSettingsCorruptTitle')}</strong>
      <p class="mt-1 text-sm">
        {i18n.t('warnSettingsCorruptBackup')}
        <code class="rounded bg-muted px-1.5 py-0.5 font-mono text-sm"
          >{settingsCorrupt.backupPath}</code
        >
      </p>
      <p class="mt-1 text-sm">{i18n.t('warnSettingsCorruptReason')}{settingsCorrupt.reason}</p>
    </section>
  {/if}

  {#if onedrive}
    <section
      class="mb-4 rounded border border-warning bg-[var(--color-warning-bg)] px-4 py-3"
    >
      <strong class="text-warning-foreground">
        {i18n.t('warnOneDriveTitle', { indicator: onedrive.detectedIndicator })}
      </strong>
      <p class="mt-1 text-sm">
        {i18n.t('warnOneDrivePath')}
        <code class="rounded bg-muted px-1.5 py-0.5 font-mono text-sm">{onedrive.dbPath}</code>
      </p>
      <p class="mt-1 text-sm">{i18n.t('warnOneDriveAdvice')}</p>
    </section>
  {/if}

  <section>
    <h2 class="mb-2 mt-6 text-lg font-medium opacity-85">{i18n.t('settingsHeading')}</h2>
    {#if loadError}
      <p class="text-destructive">{i18n.t('settingsLoadFailed')} {loadError}</p>
    {:else if settings}
      <pre class="overflow-x-auto rounded bg-muted p-3 font-mono text-sm">{JSON.stringify(
          settings,
          null,
          2
        )}</pre>
    {:else}
      <p class="text-sm opacity-55">{i18n.t('loading')}</p>
    {/if}
  </section>
</main>
