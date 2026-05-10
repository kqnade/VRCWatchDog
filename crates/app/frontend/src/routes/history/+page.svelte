<script lang="ts">
  import { convertFileSrc } from '@tauri-apps/api/core';
  import { onMount, tick } from 'svelte';
  import { page } from '$app/stores';
  import { ChevronRight, Clock, Image as ImageIcon, MapPin, Users } from 'lucide-svelte';
  import {
    listPhotosForVisit,
    listPlayersForVisit,
    listRecentVisits,
    openPhoto,
  } from '$lib/api/commands';
  import { i18n } from '$lib/i18n/use_t.svelte';
  import Badge from '$lib/ui/Badge.svelte';
  import PageHeader from '$lib/ui/PageHeader.svelte';
  import Skeleton from '$lib/ui/Skeleton.svelte';
  import type { PhotoRecord, PlayerSession, Visit } from '$lib/api/types';

  // Phase C: split view。左ペイン visit リスト、右ペイン visit 詳細。
  let visits = $state<Visit[]>([]);
  let loadError = $state<string | null>(null);
  let isLoading = $state(true);
  let selectedVisitId = $state<number | null>(null);

  let photoCache = $state<Record<number, PhotoRecord[]>>({});
  let playerCache = $state<Record<number, PlayerSession[]>>({});
  let detailError = $state<string | null>(null);

  const PAGE_SIZE = 200;
  const PHOTOS_PER_VISIT = 24;
  const PLAYERS_PER_VISIT = 200;

  const selectedVisit = $derived(visits.find((v) => v.id === selectedVisitId) ?? null);

  function badgeVariant(state: string): 'success' | 'warning' | 'destructive' | 'secondary' | 'default' {
    switch (state) {
      case 'Resolved':
        return 'success';
      case 'Pending':
        return 'warning';
      case 'MissingJoin':
        return 'warning';
      case 'ClosedWithoutJoin':
        return 'secondary';
      case 'Conflict':
        return 'destructive';
      default:
        return 'secondary';
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

  function formatShort(iso: string): string {
    const d = new Date(iso);
    const mm = String(d.getMonth() + 1).padStart(2, '0');
    const dd = String(d.getDate()).padStart(2, '0');
    const hh = String(d.getHours()).padStart(2, '0');
    const mi = String(d.getMinutes()).padStart(2, '0');
    return `${mm}/${dd} ${hh}:${mi}`;
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

  async function ensureDetailLoaded(v: Visit) {
    detailError = null;
    const tasks: Promise<void>[] = [];
    if (v.photoCount > 0 && photoCache[v.id] === undefined) {
      tasks.push(
        listPhotosForVisit(v.id, PHOTOS_PER_VISIT)
          .then((p) => {
            photoCache = { ...photoCache, [v.id]: p };
          })
          .catch((e) => {
            detailError = `${e}`;
          })
      );
    } else if (v.photoCount === 0) {
      photoCache[v.id] = [];
    }
    if (v.playerCount > 0 && playerCache[v.id] === undefined) {
      tasks.push(
        listPlayersForVisit(v.id, PLAYERS_PER_VISIT)
          .then((p) => {
            playerCache = { ...playerCache, [v.id]: p };
          })
          .catch((e) => {
            detailError = `${e}`;
          })
      );
    } else if (v.playerCount === 0) {
      playerCache[v.id] = [];
    }
    await Promise.all(tasks);
  }

  async function selectVisit(v: Visit) {
    selectedVisitId = v.id;
    await ensureDetailLoaded(v);
  }

  async function autoSelectFromUrl() {
    const param = $page.url.searchParams.get('visit');
    if (!param) return;
    const id = Number(param);
    if (!Number.isFinite(id)) return;
    const v = visits.find((x) => x.id === id);
    if (!v) return;
    await selectVisit(v);
    await tick();
    document.getElementById(`visit-${id}`)?.scrollIntoView({ block: 'center' });
  }

  async function handleOpenPhoto(p: PhotoRecord) {
    try {
      await openPhoto(p.filePath);
    } catch (e) {
      detailError = `${i18n.t('photoOpenError')} ${e}`;
    }
  }

  onMount(async () => {
    await load();
    await autoSelectFromUrl();
    // 何も選択されてない & visits があれば先頭を自動選択
    if (selectedVisitId === null && visits.length > 0) {
      await selectVisit(visits[0]);
    }
  });
</script>

<PageHeader
  title={i18n.t('historyTitle')}
  description={isLoading
    ? i18n.t('loading')
    : i18n.t('photosCountFormat', { count: visits.length, max: PAGE_SIZE })}
/>

{#if loadError}
  <div class="mb-4 rounded-md border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive">
    {loadError}
  </div>
{/if}

{#if !isLoading && visits.length === 0 && !loadError}
  <p class="text-sm text-muted-foreground">{i18n.t('historyEmpty')}</p>
{:else}
  <div class="grid gap-4 lg:grid-cols-[20rem_1fr]">
    <!-- left: visit list -->
    <div class="overflow-hidden rounded-lg border border-border bg-card">
      <div class="max-h-[calc(100vh-12rem)] overflow-y-auto">
        {#if isLoading}
          <div class="space-y-2 p-2">
            {#each Array(6) as _, i (i)}
              <Skeleton class="h-16 w-full" />
            {/each}
          </div>
        {:else}
          <ul>
            {#each visits as v (v.id)}
              {@const isSelected = v.id === selectedVisitId}
              <li>
                <button
                  type="button"
                  id="visit-{v.id}"
                  onclick={() => selectVisit(v)}
                  class="flex w-full items-center gap-2 border-l-2 px-3 py-2.5 text-left transition {isSelected
                    ? 'border-primary bg-primary/5'
                    : 'border-transparent hover:bg-muted/40'}"
                >
                  <div class="min-w-0 flex-1">
                    <p class="truncate text-sm font-medium">{v.worldName}</p>
                    <p class="mt-0.5 flex items-center gap-2 text-[11px] text-muted-foreground">
                      <Clock size={10} />{formatShort(v.joinedUtc)}
                      <span class="opacity-50">·</span>
                      <span class="font-mono">{v.duration}</span>
                    </p>
                  </div>
                  <div class="flex shrink-0 items-center gap-1.5">
                    {#if v.photoCount > 0}
                      <span class="flex items-center gap-0.5 text-[10px] text-muted-foreground">
                        <ImageIcon size={10} />{v.photoCount}
                      </span>
                    {/if}
                    {#if v.playerCount > 0}
                      <span class="flex items-center gap-0.5 text-[10px] text-muted-foreground">
                        <Users size={10} />{v.playerCount}
                      </span>
                    {/if}
                    <ChevronRight size={12} class="text-muted-foreground" />
                  </div>
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    </div>

    <!-- right: detail -->
    <div class="overflow-hidden rounded-lg border border-border bg-card">
      {#if selectedVisit}
        {@const photos = photoCache[selectedVisit.id]}
        {@const players = playerCache[selectedVisit.id]}
        <div class="max-h-[calc(100vh-12rem)] overflow-y-auto p-5">
          <header class="mb-4 flex items-start justify-between gap-3">
            <div class="min-w-0">
              <h2 class="truncate text-lg font-semibold">{selectedVisit.worldName}</h2>
              {#if selectedVisit.worldId}
                <p class="mt-1 truncate font-mono text-[11px] text-muted-foreground" title={selectedVisit.worldId}>
                  <MapPin size={10} class="inline" />{selectedVisit.worldId}
                </p>
              {/if}
            </div>
            <Badge variant={badgeVariant(selectedVisit.resolutionState)}>
              {selectedVisit.resolutionState}
            </Badge>
          </header>

          <div class="mb-5 grid grid-cols-4 gap-3 rounded-md border border-border bg-muted/30 p-3 text-xs">
            <div>
              <p class="text-[10px] uppercase tracking-wider text-muted-foreground">{i18n.t('historyJoined')}</p>
              <p class="mt-0.5 font-mono">{formatJoined(selectedVisit.joinedUtc)}</p>
            </div>
            <div>
              <p class="text-[10px] uppercase tracking-wider text-muted-foreground">{i18n.t('historyDuration')}</p>
              <p class="mt-0.5 font-mono">{selectedVisit.duration}</p>
            </div>
            <div>
              <p class="text-[10px] uppercase tracking-wider text-muted-foreground">{i18n.t('historyPhotos')}</p>
              <p class="mt-0.5 font-mono">{selectedVisit.photoCount}</p>
            </div>
            <div>
              <p class="text-[10px] uppercase tracking-wider text-muted-foreground">{i18n.t('historyPlayers')}</p>
              <p class="mt-0.5 font-mono">{selectedVisit.playerCount}</p>
            </div>
          </div>

          {#if detailError}
            <p class="mb-3 rounded border border-destructive/40 bg-destructive/10 px-2 py-1.5 text-xs text-destructive">
              {detailError}
            </p>
          {/if}

          <!-- players -->
          <section class="mb-5">
            <h3 class="mb-2 flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wider text-muted-foreground">
              <Users size={12} />{i18n.t('playersHeading', { count: selectedVisit.playerCount })}
            </h3>
            {#if selectedVisit.playerCount === 0}
              <p class="text-xs text-muted-foreground">{i18n.t('playersEmpty')}</p>
            {:else if players === undefined}
              <p class="text-xs text-muted-foreground">{i18n.t('playersLoading')}</p>
            {:else}
              <div class="flex flex-wrap gap-1.5">
                {#each players as p (p.id)}
                  <span
                    class="rounded-md bg-muted px-2 py-0.5 font-mono text-[11px] text-muted-foreground"
                    title={p.userId
                      ? `${p.userId} · joined ${formatJoined(p.joinedUtc)}${p.leftUtc ? ` / left ${formatJoined(p.leftUtc)}` : ''}`
                      : `joined ${formatJoined(p.joinedUtc)}`}
                  >
                    {p.displayName}
                  </span>
                {/each}
              </div>
            {/if}
          </section>

          <!-- photos -->
          <section>
            <h3 class="mb-2 flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wider text-muted-foreground">
              <ImageIcon size={12} />{i18n.t('photosTitle')} ({selectedVisit.photoCount})
            </h3>
            {#if selectedVisit.photoCount === 0}
              <p class="text-xs text-muted-foreground">{i18n.t('photosNoneInVisit')}</p>
            {:else if photos === undefined}
              <p class="text-xs text-muted-foreground">{i18n.t('photosLoading')}</p>
            {:else}
              <div
                class="grid gap-2"
                style="grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));"
              >
                {#each photos as photo (photo.id)}
                  <button
                    type="button"
                    class="group flex aspect-video items-center justify-center overflow-hidden rounded-md bg-muted text-[10px] text-muted-foreground transition hover:ring-2 hover:ring-primary"
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
                      {i18n.t('photoNoThumb')}
                    {/if}
                  </button>
                {/each}
              </div>
              {#if selectedVisit.photoCount > photos.length}
                <p class="mt-2 text-[11px] text-muted-foreground">
                  ({photos.length} / {selectedVisit.photoCount})
                </p>
              {/if}
            {/if}
          </section>
        </div>
      {:else}
        <div class="flex h-64 items-center justify-center text-sm text-muted-foreground">
          ←
        </div>
      {/if}
    </div>
  </div>
{/if}
