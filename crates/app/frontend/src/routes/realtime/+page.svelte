<script lang="ts">
  import { onMount } from 'svelte';
  import { MapPin, Pause, Play, Radio, Trash2, Users } from 'lucide-svelte';
  import { getRealtimeState } from '$lib/api/commands';
  import { onLiveLogEvent } from '$lib/api/events';
  import { i18n } from '$lib/i18n/use_t.svelte';
  import { session, REALTIME_LOG_MAX } from '$lib/state/session.svelte';
  import Badge from '$lib/ui/Badge.svelte';
  import Button from '$lib/ui/Button.svelte';
  import Card from '$lib/ui/Card.svelte';
  import PageHeader from '$lib/ui/PageHeader.svelte';
  import type { LiveLogEvent } from '$lib/api/types';

  // currentWorld / presence は page-local + mount で re-seed (= ページ移動で更新)。
  // log feed (events) は session.realtimeEventLog から読む (= ページ遷移しても残る)。
  let currentWorldName = $state<string | null>(null);
  let currentWorldId = $state<string | null>(null);
  let currentInstanceId = $state<string | null>(null);
  let presentPlayers = $state<string[]>([]);

  function applyEventForState(ev: LiveLogEvent): void {
    // events 配列への push は layout の listener が session に対して行うため、
    // ここでは state (currentWorld / presence) のみ更新する。
    switch (ev.kind) {
      case 'worldEntering':
        currentWorldName = ev.worldName;
        currentWorldId = null;
        currentInstanceId = null;
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
    }
  }

  function badgeVariant(kind: LiveLogEvent['kind']): 'default' | 'success' | 'warning' | 'secondary' {
    switch (kind) {
      case 'worldEntering':
      case 'worldJoining':
        return 'default';
      case 'playerJoined':
        return 'success';
      case 'playerLeft':
        return 'warning';
      default:
        return 'secondary';
    }
  }

  function describe(ev: LiveLogEvent): string {
    switch (ev.kind) {
      case 'worldEntering':
        return `→ ${ev.worldName}`;
      case 'worldJoining':
        return `${ev.worldId} / ${ev.instanceId}`;
      case 'playerJoined':
      case 'playerLeft':
        return ev.userId ? `${ev.displayName} (${ev.userId})` : ev.displayName;
      case 'notification':
        return `${ev.ntype} from ${ev.sender}`;
      case 'videoUrl':
        return ev.url;
    }
  }

  onMount(() => {
    let unlisten: (() => void) | undefined;
    // Seed: app 起動時に既に VRChat 動作中だった場合、catch-up 中の LiveLogEvent は
    // listener attach 前に流れているので、現在の active visit を DB から pull する。
    getRealtimeState()
      .then((s) => {
        if (s.currentWorld) {
          currentWorldName = s.currentWorld.worldName;
          currentWorldId = s.currentWorld.worldId;
          currentInstanceId = s.currentWorld.instanceId;
        }
        if (s.players.length > 0) {
          presentPlayers = s.players;
        }
      })
      .catch(() => {});
    // page-local listener: currentWorld / presence のみ更新。
    // events array への append は layout の listener が session に対して実施。
    onLiveLogEvent(applyEventForState).then((u) => {
      unlisten = u;
    });
    return () => unlisten?.();
  });
</script>

<PageHeader
  title={i18n.t('realtimeTitle')}
  description={i18n.t('photosCountFormat', {
    count: session.realtimeEventLog.length,
    max: REALTIME_LOG_MAX,
  })}
/>

<div class="mb-4 grid gap-4 lg:grid-cols-2">
  <Card>
    {#snippet header()}
      <div class="flex items-center gap-2">
        <MapPin size={14} class="text-primary" />
        <h2 class="text-sm font-semibold">{i18n.t('realtimeWorld')}</h2>
      </div>
    {/snippet}
    {#if currentWorldName}
      <p class="text-base font-medium">{currentWorldName}</p>
      {#if currentWorldId}
        <p class="mt-1 truncate font-mono text-xs text-muted-foreground" title={currentWorldId}>
          {currentWorldId}
        </p>
      {/if}
      {#if currentInstanceId}
        <p class="truncate font-mono text-xs text-muted-foreground" title={currentInstanceId}>
          {currentInstanceId}
        </p>
      {/if}
    {:else}
      <p class="text-sm text-muted-foreground">{i18n.t('realtimeWorldUnknown')}</p>
    {/if}
  </Card>

  <Card>
    {#snippet header()}
      <div class="flex items-center justify-between">
        <div class="flex items-center gap-2">
          <Users size={14} class="text-primary" />
          <h2 class="text-sm font-semibold">{i18n.t('realtimePresence')}</h2>
        </div>
        <Badge variant="secondary">{presentPlayers.length}</Badge>
      </div>
    {/snippet}
    {#if presentPlayers.length === 0}
      <p class="text-sm text-muted-foreground">{i18n.t('realtimePresenceEmpty')}</p>
    {:else}
      <div class="flex flex-wrap gap-1.5">
        {#each presentPlayers as name (name)}
          <span class="rounded-md bg-muted px-2 py-0.5 font-mono text-[11px] text-muted-foreground">
            {name}
          </span>
        {/each}
      </div>
    {/if}
  </Card>
</div>

<Card>
  {#snippet header()}
    <div class="flex items-center justify-between">
      <div class="flex items-center gap-2">
        <Radio size={14} class="text-primary" />
        <h2 class="text-sm font-semibold">{i18n.t('realtimeFeedHeading')}</h2>
      </div>
      <div class="flex items-center gap-1">
        <Button variant="ghost" size="sm" onclick={() => session.toggleRealtimePause()}>
          {#if session.realtimePaused}
            <Play size={12} />{i18n.t('realtimeResumeBtn')}
          {:else}
            <Pause size={12} />{i18n.t('realtimePauseBtn')}
          {/if}
        </Button>
        <Button variant="ghost" size="sm" onclick={() => session.clearRealtimeLog()}>
          <Trash2 size={12} />{i18n.t('realtimeClearBtn')}
        </Button>
      </div>
    </div>
  {/snippet}
  {#if session.realtimeEventLog.length === 0}
    <p class="py-6 text-center text-sm text-muted-foreground">{i18n.t('realtimeWaiting')}</p>
  {:else}
    <ul class="-mx-5 -my-4 max-h-[60vh] divide-y divide-border overflow-y-auto">
      {#each session.realtimeEventLog as entry (entry.seq)}
        <li class="flex items-center gap-3 px-5 py-1.5 text-xs">
          <span class="shrink-0 font-mono text-[10px] text-muted-foreground">{entry.event.naiveLocal}</span>
          <Badge variant={badgeVariant(entry.event.kind)}>{entry.event.kind}</Badge>
          <span class="truncate">{describe(entry.event)}</span>
        </li>
      {/each}
    </ul>
  {/if}
</Card>
