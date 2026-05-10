<script lang="ts">
  import { onMount } from 'svelte';
  import {
    Activity,
    AlertTriangle,
    Database,
    HardDrive,
    Heart,
    Image as ImageIcon,
    History as HistoryIcon,
    Radio,
    Timer,
    Video,
  } from 'lucide-svelte';
  import {
    listRecentNotifications,
    listRecentPhotos,
    listRecentVideos,
    listRecentVisits,
  } from '$lib/api/commands';
  import { i18n } from '$lib/i18n/use_t.svelte';
  import { session } from '$lib/state/session.svelte';
  import Badge from '$lib/ui/Badge.svelte';
  import Card from '$lib/ui/Card.svelte';
  import PageHeader from '$lib/ui/PageHeader.svelte';
  import Skeleton from '$lib/ui/Skeleton.svelte';

  // Phase C fix: health / 警告 / self は session store (= layout で初期化、永続) から
  // 読む。タブ間遷移で `null → 値` のフラッシュが起きないようにする。
  let stats = $state<{
    visits: number | null;
    photos: number | null;
    videos: number | null;
    notifications: number | null;
  }>({ visits: null, photos: null, videos: null, notifications: null });

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

  function levelVariant(level: string | undefined): 'success' | 'warning' | 'destructive' | 'secondary' {
    switch (level) {
      case 'healthy':
        return 'success';
      case 'warning':
        return 'warning';
      case 'degraded':
        return 'destructive';
      default:
        return 'secondary';
    }
  }

  type Tile = {
    href: string;
    label: string;
    icon: typeof Radio;
    count: number | null;
  };
  const tiles = $derived<Tile[]>([
    { href: '/realtime', label: i18n.t('navRealtime'), icon: Radio, count: null },
    { href: '/history', label: i18n.t('navHistory'), icon: HistoryIcon, count: stats.visits },
    { href: '/photos', label: i18n.t('navPhotos'), icon: ImageIcon, count: stats.photos },
    { href: '/videos', label: i18n.t('navVideos'), icon: Video, count: stats.videos },
    {
      href: '/notifications',
      label: i18n.t('navNotifications'),
      icon: Activity,
      count: stats.notifications,
    },
  ]);

  onMount(() => {
    Promise.all([
      listRecentVisits(1000).then((v) => v.length).catch(() => 0),
      listRecentPhotos(1000).then((p) => p.length).catch(() => 0),
      listRecentVideos(1000).then((v) => v.length).catch(() => 0),
      listRecentNotifications(1000).then((n) => n.length).catch(() => 0),
    ]).then(([visits, photos, videos, notifications]) => {
      stats = { visits, photos, videos, notifications };
    });
  });
</script>

<PageHeader title={i18n.t('appName')} description={i18n.t('homeSubtitle')} />

{#if session.settingsCorrupt}
  <div class="mb-4 flex items-start gap-3 rounded-lg border border-warning bg-warning-bg/40 p-4 text-warning-foreground">
    <AlertTriangle size={18} class="mt-0.5 shrink-0" />
    <div class="space-y-1 text-sm">
      <p class="font-semibold">{i18n.t('warnSettingsCorruptTitle')}</p>
      <p>
        {i18n.t('warnSettingsCorruptBackup')}
        <code class="rounded bg-muted px-1.5 py-0.5 font-mono text-xs">{session.settingsCorrupt.backupPath}</code>
      </p>
      <p>{i18n.t('warnSettingsCorruptReason')}{session.settingsCorrupt.reason}</p>
    </div>
  </div>
{/if}

{#if session.onedrive}
  <div class="mb-4 flex items-start gap-3 rounded-lg border border-warning bg-warning-bg/40 p-4 text-warning-foreground">
    <AlertTriangle size={18} class="mt-0.5 shrink-0" />
    <div class="space-y-1 text-sm">
      <p class="font-semibold">{i18n.t('warnOneDriveTitle', { indicator: session.onedrive.detectedIndicator })}</p>
      <p>
        {i18n.t('warnOneDrivePath')}
        <code class="rounded bg-muted px-1.5 py-0.5 font-mono text-xs">{session.onedrive.dbPath}</code>
      </p>
      <p>{i18n.t('warnOneDriveAdvice')}</p>
    </div>
  </div>
{/if}

<div class="grid gap-5 lg:grid-cols-[2fr_1fr]">
  <!-- Health -->
  <Card>
    {#snippet header()}
      <div class="flex items-center justify-between">
        <div class="flex items-center gap-2">
          <Heart size={16} class="text-primary" />
          <h2 class="text-sm font-semibold">{i18n.t('healthHeading')}</h2>
        </div>
        {#if session.health}
          <Badge variant={levelVariant(session.health.level)}>{session.health.level}</Badge>
        {:else}
          <Skeleton class="h-5 w-16" />
        {/if}
      </div>
    {/snippet}
    {#if session.health}
      <div class="grid grid-cols-4 gap-4">
        <div>
          <div class="flex items-center gap-1 text-[11px] uppercase tracking-wider text-muted-foreground">
            <Timer size={10} />{i18n.t('healthBacklog')}
          </div>
          <div class="mt-1 font-mono text-lg">{session.health.backlogSize.toLocaleString()}</div>
        </div>
        <div>
          <div class="flex items-center gap-1 text-[11px] uppercase tracking-wider text-muted-foreground">
            <Timer size={10} />{i18n.t('healthLag')}
          </div>
          <div class="mt-1 font-mono text-lg">{session.health.projectorLagSec}s</div>
        </div>
        <div>
          <div class="flex items-center gap-1 text-[11px] uppercase tracking-wider text-muted-foreground">
            <Database size={10} />{i18n.t('healthDbSize')}
          </div>
          <div class="mt-1 font-mono text-lg">{formatBytes(session.health.dbSizeBytes)}</div>
        </div>
        <div>
          <div class="flex items-center gap-1 text-[11px] uppercase tracking-wider text-muted-foreground">
            <HardDrive size={10} />{i18n.t('healthFreeDisk')}
          </div>
          <div class="mt-1 font-mono text-lg">{formatBytes(session.health.freeDiskBytes)}</div>
        </div>
      </div>
    {:else}
      <p class="text-sm text-muted-foreground">{i18n.t('healthWaiting')}</p>
    {/if}
  </Card>

  <!-- Quick stats grid -->
  <div class="grid grid-cols-2 gap-3 lg:grid-cols-1 xl:grid-cols-2">
    {#each tiles as tile (tile.href)}
      {@const Icon = tile.icon}
      <a
        href={tile.href}
        class="group flex items-center justify-between rounded-lg border border-border bg-card p-4 transition-colors hover:border-primary/50 hover:bg-muted/40"
      >
        <div class="flex items-center gap-3">
          <div class="flex h-9 w-9 items-center justify-center rounded-md bg-primary/10 text-primary transition-colors group-hover:bg-primary/15">
            <Icon size={16} />
          </div>
          <span class="text-sm font-medium">{tile.label}</span>
        </div>
        {#if tile.count !== null}
          <span class="font-mono text-base text-muted-foreground">
            {tile.count.toLocaleString()}
          </span>
        {/if}
      </a>
    {/each}
  </div>
</div>
