<script lang="ts">
  import { onMount } from 'svelte';
  import { listRecentNotifications } from '$lib/api/commands';
  import type { Notification } from '$lib/api/types';

  // Phase 7.2 + A3: notification_records の最新一覧。
  // - timeline (時系列) と by-sender (送信者集約) の 2 view を切替可能。
  // - notification_type で多選択フィルタ (chips)、count バッジ付き。
  let notifications = $state<Notification[]>([]);
  let loadError = $state<string | null>(null);
  let isLoading = $state(true);

  let viewMode = $state<'timeline' | 'sender'>('timeline');
  /** 選択中の type フィルタ。空 set = 全件表示。 */
  let selectedTypes = $state<Set<string>>(new Set());

  const PAGE_SIZE = 200;

  function badgeClass(type: string): string {
    switch (type.toLowerCase()) {
      case 'invite':
        return 'bg-muted text-muted-foreground';
      case 'requestinvite':
        return 'bg-warning-bg text-warning-foreground';
      case 'friendrequest':
        return 'bg-muted text-success';
      case 'boop':
        return 'bg-muted text-muted-foreground';
      default:
        return 'bg-muted text-muted-foreground';
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

  // 全 notification の type 別件数 (chip バッジに表示)。
  const typeCounts = $derived.by(() => {
    const m = new Map<string, number>();
    for (const n of notifications) {
      m.set(n.notificationType, (m.get(n.notificationType) ?? 0) + 1);
    }
    return [...m.entries()].sort((a, b) => b[1] - a[1]);
  });

  // selectedTypes でフィルタした array (両 view が共有)。
  const filtered = $derived(
    selectedTypes.size === 0
      ? notifications
      : notifications.filter((n) => selectedTypes.has(n.notificationType))
  );

  // sender 集約 view 用: senderName ごとに count + types + 最新 receivedUtc。
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

<main class="mx-auto min-h-screen max-w-3xl p-8">
  <header class="mb-6 flex items-baseline justify-between">
    <div>
      <h1 class="text-2xl font-semibold">Notifications</h1>
      <p class="mt-1 text-sm opacity-60">
        {#if isLoading}
          読込中…
        {:else}
          {filtered.length} 件 / 直近 {notifications.length} 件 (最大 {PAGE_SIZE} 件)
        {/if}
      </p>
    </div>
    <a href="/" class="text-sm text-muted-foreground hover:underline">← Home</a>
  </header>

  {#if loadError}
    <p class="mb-4 rounded border border-destructive bg-card px-3 py-2 text-sm text-destructive">
      {loadError}
    </p>
  {/if}

  {#if !isLoading && notifications.length > 0}
    <div class="mb-4 flex flex-wrap items-center gap-2">
      <!-- view mode toggle -->
      <div class="inline-flex overflow-hidden rounded-md border text-xs">
        <button
          type="button"
          class="px-3 py-1 transition {viewMode === 'timeline'
            ? 'bg-muted text-foreground'
            : 'bg-card text-muted-foreground hover:bg-muted/50'}"
          onclick={() => (viewMode = 'timeline')}
        >
          時系列
        </button>
        <button
          type="button"
          class="px-3 py-1 border-l transition {viewMode === 'sender'
            ? 'bg-muted text-foreground'
            : 'bg-card text-muted-foreground hover:bg-muted/50'}"
          onclick={() => (viewMode = 'sender')}
        >
          送信者ごと
        </button>
      </div>

      <!-- type filter chips -->
      <div class="flex flex-wrap items-center gap-1">
        {#each typeCounts as [type, count] (type)}
          {@const active = selectedTypes.has(type)}
          <button
            type="button"
            class="rounded-full border px-2 py-0.5 font-mono text-[11px] transition {active
              ? 'border-ring bg-muted text-foreground'
              : 'border-transparent bg-card text-muted-foreground hover:bg-muted/50'}"
            onclick={() => toggleType(type)}
            title={active ? 'クリックで解除' : 'クリックで絞り込み'}
          >
            {type} <span class="opacity-60">({count})</span>
          </button>
        {/each}
        {#if selectedTypes.size > 0}
          <button
            type="button"
            class="text-[11px] text-muted-foreground hover:underline"
            onclick={clearFilter}
          >
            clear
          </button>
        {/if}
      </div>
    </div>
  {/if}

  {#if !isLoading && notifications.length === 0 && !loadError}
    <p class="text-sm opacity-55">
      通知ログがまだ記録されていません。VRChat の動作を待ってください。
    </p>
  {:else if !isLoading && filtered.length === 0}
    <p class="text-sm opacity-55">フィルタにマッチする通知はありません。</p>
  {/if}

  {#if viewMode === 'timeline'}
    <ul class="space-y-1.5">
      {#each filtered as n (n.id)}
        <li
          class="flex items-center justify-between gap-3 rounded-md border bg-card px-3 py-2"
        >
          <div class="flex min-w-0 items-center gap-3">
            <span
              class="shrink-0 rounded px-2 py-0.5 font-mono text-xs {badgeClass(
                n.notificationType
              )}"
            >
              {n.notificationType}
            </span>
            <span class="truncate text-sm" title={n.senderName}>{n.senderName}</span>
          </div>
          <div class="flex shrink-0 items-center gap-3 text-xs opacity-60">
            {#if n.worldVisitId}
              <a
                href="/history?visit={n.worldVisitId}"
                class="font-mono hover:underline"
                title="紐づく visit を /history で開く"
              >
                visit #{n.worldVisitId}
              </a>
            {/if}
            <span class="font-mono">{formatTime(n.receivedUtc)}</span>
          </div>
        </li>
      {/each}
    </ul>
  {:else}
    <ul class="space-y-1.5">
      {#each bySender as g (g.senderName)}
        <li class="rounded-md border bg-card px-3 py-2">
          <div class="flex items-baseline justify-between gap-3">
            <span class="truncate text-sm font-medium" title={g.senderName}>
              {g.senderName}
            </span>
            <span class="shrink-0 font-mono text-xs opacity-60">
              {g.count} 件 · 最新 {formatTime(g.latestUtc)}
            </span>
          </div>
          <div class="mt-1.5 flex flex-wrap items-center gap-1">
            {#each [...g.types.entries()] as [type, count] (type)}
              <span
                class="rounded px-1.5 py-0.5 font-mono text-[11px] {badgeClass(type)}"
              >
                {type} <span class="opacity-60">×{count}</span>
              </span>
            {/each}
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</main>
