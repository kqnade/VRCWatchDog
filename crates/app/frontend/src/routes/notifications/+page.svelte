<script lang="ts">
  import { onMount } from 'svelte';
  import { Bell, MapPin, X } from 'lucide-svelte';
  import { listRecentNotifications } from '$lib/api/commands';
  import { i18n } from '$lib/i18n/use_t.svelte';
  import Badge from '$lib/ui/Badge.svelte';
  import Card from '$lib/ui/Card.svelte';
  import PageHeader from '$lib/ui/PageHeader.svelte';
  import type { Notification } from '$lib/api/types';

  let notifications = $state<Notification[]>([]);
  let loadError = $state<string | null>(null);
  let isLoading = $state(true);

  let viewMode = $state<'timeline' | 'sender'>('timeline');
  let selectedTypes = $state<Set<string>>(new Set());

  const PAGE_SIZE = 200;

  function badgeVariant(t: string): 'default' | 'success' | 'warning' | 'secondary' {
    switch (t.toLowerCase()) {
      case 'invite':
        return 'default';
      case 'requestinvite':
        return 'warning';
      case 'friendrequest':
        return 'success';
      default:
        return 'secondary';
    }
  }

  function formatTime(iso: string): string {
    const d = new Date(iso);
    const yyyy = d.getFullYear();
    const mm = String(d.getMonth() + 1).padStart(2, '0');
    const dd = String(d.getDate()).padStart(2, '0');
    const hh = String(d.getHours()).padStart(2, '0');
    const mi = String(d.getMinutes()).padStart(2, '0');
    return `${yyyy}-${mm}-${dd} ${hh}:${mi}`;
  }

  const typeCounts = $derived.by(() => {
    const m = new Map<string, number>();
    for (const n of notifications) m.set(n.notificationType, (m.get(n.notificationType) ?? 0) + 1);
    return [...m.entries()].sort((a, b) => b[1] - a[1]);
  });

  const filtered = $derived(
    selectedTypes.size === 0
      ? notifications
      : notifications.filter((n) => selectedTypes.has(n.notificationType))
  );

  type SenderGroup = {
    senderName: string;
    count: number;
    types: Map<string, number>;
    latestUtc: string;
  };
  const bySender = $derived.by<SenderGroup[]>(() => {
    const m = new Map<string, SenderGroup>();
    for (const n of filtered) {
      const g = m.get(n.senderName);
      if (g) {
        g.count += 1;
        g.types.set(n.notificationType, (g.types.get(n.notificationType) ?? 0) + 1);
        if (n.receivedUtc > g.latestUtc) g.latestUtc = n.receivedUtc;
      } else {
        m.set(n.senderName, {
          senderName: n.senderName,
          count: 1,
          types: new Map([[n.notificationType, 1]]),
          latestUtc: n.receivedUtc,
        });
      }
    }
    return [...m.values()].sort((a, b) => (a.latestUtc < b.latestUtc ? 1 : -1));
  });

  function toggleType(type: string): void {
    const next = new Set(selectedTypes);
    if (next.has(type)) next.delete(type);
    else next.add(type);
    selectedTypes = next;
  }
  function clearFilter(): void {
    selectedTypes = new Set();
  }

  async function load() {
    isLoading = true;
    try {
      notifications = await listRecentNotifications(PAGE_SIZE);
      loadError = null;
    } catch (e) {
      loadError = String(e);
    } finally {
      isLoading = false;
    }
  }

  onMount(() => {
    void load();
  });
</script>

<PageHeader
  title={i18n.t('notificationsTitle')}
  description={isLoading
    ? i18n.t('loading')
    : `${filtered.length} / ${i18n.t('photosCountFormat', { count: notifications.length, max: PAGE_SIZE })}`}
/>

{#if loadError}
  <div class="mb-4 rounded-md border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive">
    {loadError}
  </div>
{/if}

{#if !isLoading && notifications.length > 0}
  <div class="mb-4 flex flex-wrap items-center gap-3">
    <!-- view mode toggle -->
    <div class="inline-flex overflow-hidden rounded-md border border-border text-xs">
      <button
        type="button"
        class="px-3 py-1.5 transition {viewMode === 'timeline'
          ? 'bg-primary text-primary-foreground'
          : 'bg-card text-muted-foreground hover:bg-muted/50'}"
        onclick={() => (viewMode = 'timeline')}
      >
        {i18n.t('notifViewTimeline')}
      </button>
      <button
        type="button"
        class="border-l border-border px-3 py-1.5 transition {viewMode === 'sender'
          ? 'bg-primary text-primary-foreground'
          : 'bg-card text-muted-foreground hover:bg-muted/50'}"
        onclick={() => (viewMode = 'sender')}
      >
        {i18n.t('notifViewBySender')}
      </button>
    </div>

    <!-- type filter chips -->
    <div class="flex flex-wrap items-center gap-1">
      {#each typeCounts as [type, count] (type)}
        {@const active = selectedTypes.has(type)}
        <button
          type="button"
          class="rounded-full border px-2.5 py-0.5 font-mono text-[11px] transition {active
            ? 'border-primary bg-primary/10 text-foreground'
            : 'border-border bg-card text-muted-foreground hover:border-primary/50'}"
          onclick={() => toggleType(type)}
        >
          {type} <span class="opacity-60">({count})</span>
        </button>
      {/each}
      {#if selectedTypes.size > 0}
        <button
          type="button"
          class="flex items-center gap-1 text-[11px] text-muted-foreground hover:text-foreground"
          onclick={clearFilter}
        >
          <X size={10} />{i18n.t('filterClear')}
        </button>
      {/if}
    </div>
  </div>
{/if}

{#if !isLoading && notifications.length === 0 && !loadError}
  <p class="text-sm text-muted-foreground">{i18n.t('notificationsEmpty')}</p>
{:else if !isLoading && filtered.length === 0}
  <p class="text-sm text-muted-foreground">{i18n.t('notificationsFilteredEmpty')}</p>
{/if}

{#if viewMode === 'timeline'}
  <Card class="overflow-hidden">
    <div class="-mx-5 -my-4 divide-y divide-border">
      {#each filtered as n (n.id)}
        <div class="flex items-center justify-between gap-3 px-5 py-2.5">
          <div class="flex min-w-0 items-center gap-3">
            <Bell size={12} class="shrink-0 text-muted-foreground" />
            <Badge variant={badgeVariant(n.notificationType)}>{n.notificationType}</Badge>
            <span class="truncate text-sm" title={n.senderName}>{n.senderName}</span>
          </div>
          <div class="flex shrink-0 items-center gap-3 text-xs text-muted-foreground">
            {#if n.worldVisitId}
              <a
                href="/history?visit={n.worldVisitId}"
                class="flex items-center gap-1 font-mono hover:text-foreground hover:underline"
                title={i18n.t('visitOpenInHistory')}
              >
                <MapPin size={10} />#{n.worldVisitId}
              </a>
            {/if}
            <span class="font-mono">{formatTime(n.receivedUtc)}</span>
          </div>
        </div>
      {/each}
    </div>
  </Card>
{:else}
  <ul class="space-y-2">
    {#each bySender as g (g.senderName)}
      <li class="rounded-lg border border-border bg-card px-4 py-3">
        <div class="flex items-baseline justify-between gap-3">
          <span class="truncate text-sm font-medium" title={g.senderName}>{g.senderName}</span>
          <span class="shrink-0 font-mono text-xs text-muted-foreground">
            {i18n.t('senderLatest', { count: g.count, time: formatTime(g.latestUtc) })}
          </span>
        </div>
        <div class="mt-2 flex flex-wrap items-center gap-1">
          {#each [...g.types.entries()] as [type, count] (type)}
            <Badge variant={badgeVariant(type)}>
              {type} <span class="ml-1 opacity-60">×{count}</span>
            </Badge>
          {/each}
        </div>
      </li>
    {/each}
  </ul>
{/if}
