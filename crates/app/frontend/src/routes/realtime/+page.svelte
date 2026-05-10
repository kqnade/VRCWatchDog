<script lang="ts">
  import { onMount } from 'svelte';
  import { onLiveLogEvent } from '$lib/api/events';
  import { i18n } from '$lib/i18n/use_t.svelte';
  import type { LiveLogEvent } from '$lib/api/types';

  // Phase B2: live_log_event を購読して
  // - ring buffer に最新 200 件のログを積む
  // - 直近の WorldEntering / WorldJoining を「現在のワールド」として表示
  // - PlayerJoined / PlayerLeft で同居プレイヤー集合を維持
  //
  // 起動前の events は取りこぼす (catch-up 分は他ページで読める)。

  const RING_SIZE = 200;
  let events = $state<LiveLogEvent[]>([]);
  let currentWorldName = $state<string | null>(null);
  let currentWorldId = $state<string | null>(null);
  let currentInstanceId = $state<string | null>(null);
  /** 現在同居しているプレイヤーの ordered set。最初に Joined した順を維持する。 */
  let presentPlayers = $state<string[]>([]);
  let isPaused = $state(false);

  function pushEvent(ev: LiveLogEvent): void {
    events = [ev, ...events].slice(0, RING_SIZE);
  }

  function applyEvent(ev: LiveLogEvent): void {
    if (!isPaused) pushEvent(ev);
    switch (ev.kind) {
      case 'worldEntering':
        currentWorldName = ev.worldName;
        currentWorldId = null;
        currentInstanceId = null;
        // 部屋を変えたら同居 players は wipe (実 join イベントで再構築される)
        presentPlayers = [];
        break;
      case 'worldJoining':
        currentWorldId = ev.worldId;
        currentInstanceId = ev.instanceId;
        break;
      case 'playerJoined':
        if (!presentPlayers.includes(ev.displayName)) {
          presentPlayers = [...presentPlayers, ev.displayName];
        }
        break;
      case 'playerLeft':
        presentPlayers = presentPlayers.filter((p) => p !== ev.displayName);
        break;
      // notification / videoUrl は state に影響しない
    }
  }

  function badgeClass(kind: LiveLogEvent['kind']): string {
    switch (kind) {
      case 'worldEntering':
      case 'worldJoining':
        return 'bg-muted text-foreground';
      case 'playerJoined':
        return 'bg-muted text-success';
      case 'playerLeft':
        return 'bg-muted text-warning-foreground';
      case 'notification':
        return 'bg-muted text-muted-foreground';
      case 'videoUrl':
        return 'bg-muted text-muted-foreground';
      default:
        return 'bg-muted text-muted-foreground';
    }
  }

  function describe(ev: LiveLogEvent): string {
    switch (ev.kind) {
      case 'worldEntering':
        return `→ ${ev.worldName}`;
      case 'worldJoining':
        return `${ev.worldId} / ${ev.instanceId}`;
      case 'playerJoined':
        return ev.userId ? `${ev.displayName} (${ev.userId})` : ev.displayName;
      case 'playerLeft':
        return ev.userId ? `${ev.displayName} (${ev.userId})` : ev.displayName;
      case 'notification':
        return `${ev.ntype} from ${ev.sender}`;
      case 'videoUrl':
        return ev.url;
    }
  }

  function clearLog(): void {
    events = [];
  }

  onMount(() => {
    let unlisten: (() => void) | undefined;
    onLiveLogEvent((p) => applyEvent(p)).then((u) => {
      unlisten = u;
    });
    return () => unlisten?.();
  });
</script>

<main class="mx-auto min-h-screen max-w-5xl p-8">
  <header class="mb-6 flex items-baseline justify-between">
    <div>
      <h1 class="text-2xl font-semibold">{i18n.t('realtimeTitle')}</h1>
      <p class="mt-1 text-sm opacity-60">
        {i18n.t('photosCountFormat', { count: events.length, max: RING_SIZE })}
      </p>
    </div>
    <a href="/" class="text-sm text-muted-foreground hover:underline">{i18n.t('navHomeBack')}</a>
  </header>

  <div class="mb-4 grid gap-4 md:grid-cols-2">
    <!-- current world -->
    <section class="rounded-md border bg-card p-4">
      <h2 class="mb-2 text-xs font-semibold uppercase tracking-wider opacity-55">
        {i18n.t('realtimeWorld')}
      </h2>
      {#if currentWorldName}
        <p class="text-base font-medium">{currentWorldName}</p>
        {#if currentWorldId}
          <p class="mt-1 truncate font-mono text-xs opacity-55" title={currentWorldId}>
            {currentWorldId}
          </p>
        {/if}
        {#if currentInstanceId}
          <p class="truncate font-mono text-xs opacity-55" title={currentInstanceId}>
            {currentInstanceId}
          </p>
        {/if}
      {:else}
        <p class="text-sm opacity-55">{i18n.t('realtimeWorldUnknown')}</p>
      {/if}
    </section>

    <!-- present players -->
    <section class="rounded-md border bg-card p-4">
      <h2 class="mb-2 flex items-baseline justify-between text-xs font-semibold uppercase tracking-wider opacity-55">
        <span>{i18n.t('realtimePresence')}</span>
        <span class="font-mono text-[11px]">{presentPlayers.length}</span>
      </h2>
      {#if presentPlayers.length === 0}
        <p class="text-sm opacity-55">{i18n.t('realtimePresenceEmpty')}</p>
      {:else}
        <div class="flex flex-wrap gap-1.5">
          {#each presentPlayers as name (name)}
            <span class="rounded bg-muted px-1.5 py-0.5 font-mono text-[11px] text-muted-foreground">
              {name}
            </span>
          {/each}
        </div>
      {/if}
    </section>
  </div>

  <!-- log feed -->
  <section class="rounded-md border bg-card">
    <div class="flex items-center justify-between border-b px-4 py-2">
      <h2 class="text-xs font-semibold uppercase tracking-wider opacity-55">
        {i18n.t('realtimeFeedHeading')}
      </h2>
      <div class="flex items-center gap-2 text-xs">
        <button
          type="button"
          class="rounded border px-2 py-0.5 text-muted-foreground transition hover:bg-muted/50"
          onclick={() => (isPaused = !isPaused)}
        >
          {isPaused ? i18n.t('realtimeResumeBtn') : i18n.t('realtimePauseBtn')}
        </button>
        <button
          type="button"
          class="rounded border px-2 py-0.5 text-muted-foreground transition hover:bg-muted/50"
          onclick={clearLog}
        >
          {i18n.t('realtimeClearBtn')}
        </button>
      </div>
    </div>
    {#if events.length === 0}
      <p class="px-4 py-6 text-center text-sm opacity-55">{i18n.t('realtimeWaiting')}</p>
    {:else}
      <ul class="max-h-[60vh] divide-y overflow-y-auto">
        {#each events as ev, i (i + ev.naiveLocal + ev.kind)}
          <li class="flex items-center gap-3 px-4 py-1.5">
            <span class="shrink-0 font-mono text-[10px] opacity-55">{ev.naiveLocal}</span>
            <span class="shrink-0 rounded px-1.5 py-0.5 font-mono text-[10px] {badgeClass(ev.kind)}">
              {ev.kind}
            </span>
            <span class="truncate text-xs">{describe(ev)}</span>
          </li>
        {/each}
      </ul>
    {/if}
  </section>
</main>
