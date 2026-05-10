<script lang="ts">
  import { onMount } from 'svelte';
  import { listRecentNotifications } from '$lib/api/commands';
  import type { Notification } from '$lib/api/types';

  // Phase 7.2: notification_records の最新一覧。
  // backend が received_utc DESC で渡してくれるので UI 側は表示だけ。
  let notifications = $state<Notification[]>([]);
  let loadError = $state<string | null>(null);
  let isLoading = $state(true);

  const PAGE_SIZE = 200;

  function badgeClass(type: string): string {
    // Notification の種類で色分け (主要なもの: invite / requestInvite / friendRequest / boop)
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
          直近 {notifications.length} 件 (最大 {PAGE_SIZE} 件)
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

  {#if !isLoading && notifications.length === 0 && !loadError}
    <p class="text-sm opacity-55">
      通知ログがまだ記録されていません。VRChat の動作を待ってください。
    </p>
  {/if}

  <ul class="space-y-1.5">
    {#each notifications as n (n.id)}
      <li class="flex items-center justify-between gap-3 rounded-md border bg-card px-3 py-2">
        <div class="flex min-w-0 items-center gap-3">
          <span
            class="shrink-0 rounded px-2 py-0.5 font-mono text-xs {badgeClass(n.notificationType)}"
          >
            {n.notificationType}
          </span>
          <span class="truncate text-sm" title={n.senderName}>{n.senderName}</span>
        </div>
        <div class="flex shrink-0 items-center gap-3 text-xs opacity-60">
          {#if n.worldVisitId}
            <span class="font-mono">visit #{n.worldVisitId}</span>
          {/if}
          <span class="font-mono">{formatTime(n.receivedUtc)}</span>
        </div>
      </li>
    {/each}
  </ul>
</main>
