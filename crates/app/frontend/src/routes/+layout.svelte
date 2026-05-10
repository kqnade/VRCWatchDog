<script lang="ts">
  // Tailwind v4 entry を全 route に流し込む。
  import '../app.css';
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import { Activity, History, Image, LayoutDashboard, Radio, Settings as SettingsIcon, User, Video } from 'lucide-svelte';
  import { getInitialWarnings, getSelfPlayer, getSettings, getThumbProgress } from '$lib/api/commands';
  import { onHealthStatus, onOneDriveWarning, onSettingsCorrupt } from '$lib/api/events';
  import { i18n } from '$lib/i18n/use_t.svelte';
  import { session } from '$lib/state/session.svelte';
  import { applyTheme } from '$lib/theme.svelte';

  let { children } = $props();

  type NavItem = {
    href: string;
    icon: typeof LayoutDashboard;
    labelKey:
      | 'navHome'
      | 'navRealtime'
      | 'navHistory'
      | 'navPhotos'
      | 'navVideos'
      | 'navNotifications'
      | 'navSettings';
  };
  const NAV: readonly NavItem[] = [
    { href: '/', icon: LayoutDashboard, labelKey: 'navHome' },
    { href: '/realtime', icon: Radio, labelKey: 'navRealtime' },
    { href: '/history', icon: History, labelKey: 'navHistory' },
    { href: '/photos', icon: Image, labelKey: 'navPhotos' },
    { href: '/videos', icon: Video, labelKey: 'navVideos' },
    { href: '/notifications', icon: Activity, labelKey: 'navNotifications' },
  ];

  function isActive(href: string, current: string): boolean {
    if (href === '/') return current === '/';
    return current === href || current.startsWith(href + '/');
  }

  const unlistens: Array<() => void> = [];

  onMount(() => {
    // settings + self は最初の 1 度だけ fetch して store に流す。
    getSettings()
      .then((s) => {
        i18n.setLocale(s.locale);
        applyTheme(s);
      })
      .catch(() => {});
    getSelfPlayer()
      .then((s) => {
        session.self = s;
      })
      .catch(() => {});

    // Phase C fix: health / 起動時警告は layout で 1 度だけ subscribe し、session store に
    // 流し込む。各 page は session.health を読むだけなので、ホームに戻るたびに
    // 再 subscribe で null → 値 へリセットされる現象が消える。
    onHealthStatus((p) => {
      session.health = p;
    }).then((u) => unlistens.push(u));
    onSettingsCorrupt((p) => {
      session.settingsCorrupt = p;
    }).then((u) => unlistens.push(u));
    onOneDriveWarning((p) => {
      session.onedrive = p;
    }).then((u) => unlistens.push(u));

    getInitialWarnings()
      .then((w) => {
        if (w.settingsCorrupt) session.settingsCorrupt = w.settingsCorrupt;
        if (w.dbSyncRisk) session.onedrive = w.dbSyncRisk;
      })
      .catch(() => {});

    // thumb 進捗 polling: 3 秒間隔。pending=0 なら間隔を 30 秒に伸ばして CPU 節約。
    let thumbTimer: ReturnType<typeof setTimeout> | null = null;
    const tick = async () => {
      try {
        const p = await getThumbProgress();
        session.thumbProgress = p;
        const next = p.pending > 0 ? 3000 : 30000;
        thumbTimer = setTimeout(tick, next);
      } catch {
        thumbTimer = setTimeout(tick, 30000);
      }
    };
    void tick();

    return () => {
      for (const u of unlistens) u();
      if (thumbTimer) clearTimeout(thumbTimer);
    };
  });
</script>

<div class="grid h-screen grid-cols-[14rem_1fr] bg-background text-foreground">
  <!-- Sidebar -->
  <aside class="flex h-screen flex-col border-r border-sidebar-border bg-sidebar text-sidebar-foreground">
    <div class="flex items-center gap-2 px-4 py-4">
      <div class="flex h-8 w-8 items-center justify-center rounded-md bg-primary text-primary-foreground">
        <Radio size={18} />
      </div>
      <div class="flex flex-col">
        <span class="text-sm font-semibold leading-none">VRCWatchDog</span>
        {#if session.self?.displayName}
          <span class="mt-1 flex items-center gap-1 truncate text-[11px] text-muted-foreground" title={session.self.authenticatedUtc ?? ''}>
            <User size={10} />@{session.self.displayName}
          </span>
        {/if}
      </div>
    </div>

    <nav class="mt-2 flex-1 space-y-0.5 px-2">
      {#each NAV as item (item.href)}
        {@const Icon = item.icon}
        {@const active = isActive(item.href, $page.url.pathname)}
        <a
          href={item.href}
          class="flex items-center gap-2.5 rounded-md px-3 py-2 text-sm transition-colors {active
            ? 'bg-sidebar-accent text-sidebar-accent-foreground font-medium'
            : 'text-muted-foreground hover:bg-sidebar-accent/50 hover:text-foreground'}"
        >
          <Icon size={16} />
          <span class="flex-1">{i18n.t(item.labelKey)}</span>
          {#if item.href === '/photos' && session.thumbProgress && session.thumbProgress.pending > 0}
            <span
              class="rounded-full bg-primary/20 px-1.5 py-0.5 font-mono text-[9px] text-primary"
              title="サムネ生成待ち {session.thumbProgress.pending} / {session.thumbProgress.total}"
            >
              {session.thumbProgress.pending}
            </span>
          {/if}
        </a>
      {/each}
    </nav>

    <div class="border-t border-sidebar-border p-2">
      <a
        href="/settings"
        class="flex items-center gap-2.5 rounded-md px-3 py-2 text-sm transition-colors {isActive(
          '/settings',
          $page.url.pathname
        )
          ? 'bg-sidebar-accent text-sidebar-accent-foreground font-medium'
          : 'text-muted-foreground hover:bg-sidebar-accent/50 hover:text-foreground'}"
      >
        <SettingsIcon size={16} />
        <span>{i18n.t('navSettings')}</span>
      </a>
    </div>
  </aside>

  <!-- Content -->
  <main class="overflow-y-auto">
    <div class="mx-auto max-w-6xl px-8 py-8">
      {@render children?.()}
    </div>
  </main>
</div>
