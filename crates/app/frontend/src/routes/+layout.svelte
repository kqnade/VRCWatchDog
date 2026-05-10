<script lang="ts">
  // Tailwind v4 entry を全 route に流し込む。
  import '../app.css';
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import { Activity, History, Image, LayoutDashboard, Radio, Settings as SettingsIcon, User, Video } from 'lucide-svelte';
  import { getSelfPlayer, getSettings } from '$lib/api/commands';
  import { i18n } from '$lib/i18n/use_t.svelte';
  import { applyTheme } from '$lib/theme.svelte';
  import type { SelfPlayer } from '$lib/api/types';

  let { children } = $props();

  let self = $state<SelfPlayer | null>(null);

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

  onMount(async () => {
    try {
      const s = await getSettings();
      i18n.setLocale(s.locale);
      applyTheme(s);
    } catch {
      // settings 未取得でも default theme で表示できる
    }
    try {
      self = await getSelfPlayer();
    } catch {
      self = null;
    }
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
        {#if self?.displayName}
          <span class="mt-1 flex items-center gap-1 truncate text-[11px] text-muted-foreground" title={self.authenticatedUtc ?? ''}>
            <User size={10} />@{self.displayName}
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
          <span>{i18n.t(item.labelKey)}</span>
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
