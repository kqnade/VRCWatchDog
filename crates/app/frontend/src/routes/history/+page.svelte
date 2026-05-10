<script lang="ts">
  import { onMount } from 'svelte';
  import { listRecentVisits } from '$lib/api/commands';
  import type { Visit } from '$lib/api/types';

  // Phase 6.4: world_visits の活動履歴。
  // backend が photo_count + duration を計算済みで返してくれるので、UI 側は表示だけ。
  let visits = $state<Visit[]>([]);
  let loadError = $state<string | null>(null);
  let isLoading = $state(true);

  const PAGE_SIZE = 100;

  function badgeClass(state: string): string {
    // resolution_state によって色分け (plan §2 の 5 状態)
    switch (state) {
      case 'Resolved':
        return 'bg-muted text-muted-foreground';
      case 'Pending':
        return 'bg-muted text-warning-foreground';
      case 'MissingJoin':
        return 'bg-warning-bg text-warning-foreground';
      case 'ClosedWithoutJoin':
        return 'bg-muted text-muted-foreground';
      case 'Conflict':
        return 'bg-destructive/20 text-destructive';
      default:
        return 'bg-muted text-muted-foreground';
    }
  }

  function formatJoined(iso: string): string {
    // backend からは ISO 8601 UTC で来る。表示は "YYYY-MM-DD HH:MM" にローカル化。
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
      visits = await listRecentVisits(PAGE_SIZE);
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

<main class="mx-auto min-h-screen max-w-4xl p-8">
  <header class="mb-6 flex items-baseline justify-between">
    <div>
      <h1 class="text-2xl font-semibold">Activity History</h1>
      <p class="mt-1 text-sm opacity-60">
        {#if isLoading}
          読込中…
        {:else}
          直近 {visits.length} 件 (最大 {PAGE_SIZE} 件)
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

  {#if !isLoading && visits.length === 0 && !loadError}
    <p class="text-sm opacity-55">
      まだ visit がありません。VRChat を起動して log_watcher が events を拾うのを待ってください。
    </p>
  {/if}

  <ul class="space-y-2">
    {#each visits as visit (visit.id)}
      <li class="rounded-md border bg-card p-3">
        <div class="flex items-baseline justify-between gap-3">
          <h2 class="truncate text-base font-medium" title={visit.worldName}>
            {visit.worldName}
          </h2>
          <span
            class="shrink-0 rounded px-2 py-0.5 text-xs font-mono {badgeClass(
              visit.resolutionState
            )}"
            title="resolution_state"
          >
            {visit.resolutionState}
          </span>
        </div>
        <div class="mt-2 grid grid-cols-3 gap-4 text-xs">
          <div>
            <span class="block uppercase tracking-wider opacity-55">Joined</span>
            <span class="font-mono">{formatJoined(visit.joinedUtc)}</span>
          </div>
          <div>
            <span class="block uppercase tracking-wider opacity-55">Duration</span>
            <span class="font-mono">{visit.duration}</span>
          </div>
          <div>
            <span class="block uppercase tracking-wider opacity-55">Photos</span>
            <span class="font-mono">{visit.photoCount}</span>
          </div>
        </div>
        {#if visit.worldId}
          <p class="mt-2 truncate font-mono text-xs opacity-55" title={visit.worldId}>
            {visit.worldId}
          </p>
        {/if}
      </li>
    {/each}
  </ul>
</main>
