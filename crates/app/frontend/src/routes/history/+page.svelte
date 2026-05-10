<script lang="ts">
  import { convertFileSrc } from '@tauri-apps/api/core';
  import { onMount, tick } from 'svelte';
  import { page } from '$app/stores';
  import { listPhotosForVisit, listRecentVisits, openPhoto } from '$lib/api/commands';
  import type { PhotoRecord, Visit } from '$lib/api/types';

  // Phase 6.4 + A2: world_visits の活動履歴。
  // - クリックで展開、紐づく写真サムネ strip を inline 表示。
  // - URL ?visit=<id> 付きで来たら自動展開 (例: /photos のカードからジャンプ)。
  let visits = $state<Visit[]>([]);
  let loadError = $state<string | null>(null);
  let isLoading = $state(true);

  // 展開中の visit id (1 件のみ展開、Map にすれば複数同時展開も可)
  let expandedVisitId = $state<number | null>(null);
  // 展開時に fetch した写真 (visit_id ごとに cache)
  let photoCache = $state<Record<number, PhotoRecord[]>>({});
  let photoLoadError = $state<string | null>(null);

  const PAGE_SIZE = 100;
  const PHOTOS_PER_VISIT = 24; // 展開時に表示する最大件数

  function badgeClass(state: string): string {
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

  async function ensurePhotosLoaded(visitId: number): Promise<void> {
    if (photoCache[visitId] !== undefined) return;
    try {
      const photos = await listPhotosForVisit(visitId, PHOTOS_PER_VISIT);
      photoCache = { ...photoCache, [visitId]: photos };
      photoLoadError = null;
    } catch (e) {
      photoLoadError = `写真読込に失敗: ${e}`;
    }
  }

  async function toggleExpand(visit: Visit): Promise<void> {
    if (expandedVisitId === visit.id) {
      expandedVisitId = null;
      return;
    }
    expandedVisitId = visit.id;
    // photoCount=0 ならわざわざ fetch しない (空 array 確定)
    if (visit.photoCount > 0) {
      await ensurePhotosLoaded(visit.id);
    } else {
      photoCache = { ...photoCache, [visit.id]: [] };
    }
  }

  function onRowKey(visit: Visit, ev: KeyboardEvent): void {
    if (ev.key === 'Enter' || ev.key === ' ') {
      ev.preventDefault();
      void toggleExpand(visit);
    }
  }

  async function handleOpenPhoto(photo: PhotoRecord): Promise<void> {
    try {
      await openPhoto(photo.filePath);
    } catch (e) {
      photoLoadError = `写真を開けませんでした: ${e}`;
    }
  }

  // URL ?visit=<id> から自動展開 + 該当行までスクロール
  async function autoExpandFromUrl(): Promise<void> {
    const param = $page.url.searchParams.get('visit');
    if (!param) return;
    const id = Number(param);
    if (!Number.isFinite(id) || id <= 0) return;
    const visit = visits.find((v) => v.id === id);
    if (!visit) return;
    expandedVisitId = id;
    if (visit.photoCount > 0) {
      await ensurePhotosLoaded(id);
    }
    await tick();
    document.getElementById(`visit-${id}`)?.scrollIntoView({ block: 'center' });
  }

  onMount(async () => {
    await load();
    await autoExpandFromUrl();
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
      {@const isExpanded = expandedVisitId === visit.id}
      {@const photos = photoCache[visit.id]}
      <li
        id="visit-{visit.id}"
        class="rounded-md border bg-card transition {isExpanded
          ? 'ring-1 ring-ring'
          : ''}"
      >
        <div
          role="button"
          tabindex="0"
          class="cursor-pointer p-3 focus:outline-none"
          onclick={() => toggleExpand(visit)}
          onkeydown={(ev) => onRowKey(visit, ev)}
        >
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
        </div>

        {#if isExpanded}
          <div class="border-t px-3 py-3">
            {#if photoLoadError}
              <p class="mb-2 text-xs text-destructive">{photoLoadError}</p>
            {/if}
            {#if visit.photoCount === 0}
              <p class="text-xs opacity-55">この visit に紐づく写真はありません</p>
            {:else if photos === undefined}
              <p class="text-xs opacity-55">読込中…</p>
            {:else if photos.length === 0}
              <p class="text-xs opacity-55">写真は 0 件です</p>
            {:else}
              <div
                class="grid gap-2"
                style="grid-template-columns: repeat(auto-fill, minmax(120px, 1fr));"
              >
                {#each photos as photo (photo.id)}
                  <button
                    type="button"
                    class="group flex aspect-video items-center justify-center overflow-hidden rounded bg-muted text-[10px] opacity-90 transition hover:opacity-100 focus:outline-none focus:ring-2 focus:ring-ring"
                    onclick={() => handleOpenPhoto(photo)}
                    title={photo.fileName}
                  >
                    {#if photo.thumbPath}
                      <img
                        src={convertFileSrc(photo.thumbPath)}
                        alt={photo.fileName}
                        class="h-full w-full object-cover"
                        loading="lazy"
                      />
                    {:else}
                      no thumb
                    {/if}
                  </button>
                {/each}
              </div>
              {#if visit.photoCount > photos.length}
                <p class="mt-2 text-[11px] opacity-55">
                  ({photos.length} / {visit.photoCount} 件表示)
                </p>
              {/if}
            {/if}
          </div>
        {/if}
      </li>
    {/each}
  </ul>
</main>
