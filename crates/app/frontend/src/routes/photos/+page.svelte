<script lang="ts">
  import { convertFileSrc } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';
  import { FolderOpen, MapPin } from 'lucide-svelte';
  import { listRecentPhotos, openPhoto, openPhotoFolder } from '$lib/api/commands';
  import { i18n } from '$lib/i18n/use_t.svelte';
  import { session } from '$lib/state/session.svelte';
  import Badge from '$lib/ui/Badge.svelte';
  import PageHeader from '$lib/ui/PageHeader.svelte';
  import Skeleton from '$lib/ui/Skeleton.svelte';
  import type { PhotoRecord } from '$lib/api/types';

  let photos = $state<PhotoRecord[]>([]);
  let loadError = $state<string | null>(null);
  let isLoading = $state(true);

  const PAGE_SIZE = 200;

  function dayKey(iso: string): string {
    const d = new Date(iso);
    const yyyy = d.getFullYear();
    const mm = String(d.getMonth() + 1).padStart(2, '0');
    const dd = String(d.getDate()).padStart(2, '0');
    return `${yyyy}-${mm}-${dd}`;
  }

  function formatDayLabel(key: string): string {
    return key;
  }

  // 撮影日 (YYYY-MM-DD) で group。新しい順を維持。
  const grouped = $derived.by<Array<{ day: string; items: PhotoRecord[] }>>(() => {
    const map = new Map<string, PhotoRecord[]>();
    for (const p of photos) {
      const k = dayKey(p.takenUtc);
      const arr = map.get(k);
      if (arr) arr.push(p);
      else map.set(k, [p]);
    }
    return [...map.entries()].map(([day, items]) => ({ day, items }));
  });

  async function load() {
    isLoading = true;
    try {
      photos = await listRecentPhotos(PAGE_SIZE);
      loadError = null;
    } catch (e) {
      loadError = String(e);
    } finally {
      isLoading = false;
    }
  }

  async function handleOpen(p: PhotoRecord) {
    try {
      await openPhoto(p.filePath);
    } catch (e) {
      loadError = `${i18n.t('photoOpenError')} ${e}`;
    }
  }

  async function handleOpenFolder(p: PhotoRecord, ev: MouseEvent | KeyboardEvent) {
    ev.stopPropagation();
    try {
      await openPhotoFolder(p.filePath);
    } catch (e) {
      loadError = `${i18n.t('photoFolderOpenError')} ${e}`;
    }
  }

  function onCardKey(p: PhotoRecord, ev: KeyboardEvent) {
    if (ev.key === 'Enter' || ev.key === ' ') {
      ev.preventDefault();
      void handleOpen(p);
    }
  }

  onMount(() => {
    void load();
    // thumb 生成中は 5 秒ごとに自動 refresh して新サムネを反映する。
    // session.thumbProgress は layout が polling して更新しているのを購読する。
    const refreshTimer = setInterval(() => {
      if ((session.thumbProgress?.pending ?? 0) > 0) {
        void load();
      }
    }, 5000);
    return () => clearInterval(refreshTimer);
  });
</script>

<PageHeader
  title={i18n.t('photosTitle')}
  description={isLoading
    ? i18n.t('loading')
    : i18n.t('photosCountFormat', { count: photos.length, max: PAGE_SIZE })}
>
  {#snippet actions()}
    {#if session.thumbProgress}
      {@const tp = session.thumbProgress}
      {#if tp.pending > 0}
        <Badge variant="default" class="animate-pulse">
          サムネ {tp.ready} / {tp.total}
        </Badge>
      {:else if tp.total > 0}
        <Badge variant="success">サムネ {tp.ready} / {tp.total}</Badge>
      {/if}
    {/if}
  {/snippet}
</PageHeader>

{#if loadError}
  <div class="mb-4 rounded-md border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive">
    {loadError}
  </div>
{/if}

{#if isLoading}
  <div class="grid gap-3" style="grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));">
    {#each Array(12) as _, i (i)}
      <div class="space-y-2">
        <Skeleton class="aspect-video w-full" />
        <Skeleton class="h-3 w-3/4" />
      </div>
    {/each}
  </div>
{:else if photos.length === 0 && !loadError}
  <p class="text-sm text-muted-foreground">{i18n.t('photosEmpty')}</p>
{:else}
  <div class="space-y-6">
    {#each grouped as group (group.day)}
      <section>
        <h2 class="sticky top-0 z-10 mb-3 -mx-1 bg-background/95 px-1 py-1 font-mono text-xs font-semibold uppercase tracking-wider text-muted-foreground backdrop-blur">
          {formatDayLabel(group.day)}
          <span class="ml-2 opacity-60">({group.items.length})</span>
        </h2>
        <div class="grid gap-3" style="grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));">
          {#each group.items as photo (photo.id)}
            <div
              role="button"
              tabindex="0"
              class="group flex cursor-pointer flex-col overflow-hidden rounded-md border border-border bg-card transition-colors hover:border-primary/50 focus:outline-none focus:ring-2 focus:ring-ring"
              onclick={() => handleOpen(photo)}
              onkeydown={(ev) => onCardKey(photo, ev)}
              title={i18n.t('photoOpenHint')}
            >
              <div class="flex aspect-video items-center justify-center overflow-hidden bg-muted text-[10px] text-muted-foreground">
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
              </div>
              <div class="flex items-center justify-between gap-1 px-2.5 py-2 text-[11px]">
                {#if photo.worldVisitId}
                  <a
                    href="/history?visit={photo.worldVisitId}"
                    class="flex min-w-0 items-center gap-1 truncate text-muted-foreground hover:text-foreground hover:underline"
                    title={photo.worldName ?? `visit #${photo.worldVisitId}`}
                    onclick={(ev) => ev.stopPropagation()}
                  >
                    <MapPin size={10} class="shrink-0" />
                    <span class="truncate">{photo.worldName ?? `#${photo.worldVisitId}`}</span>
                  </a>
                {:else}
                  <span class="text-muted-foreground/40">{i18n.t('photoNoVisit')}</span>
                {/if}
                <button
                  type="button"
                  class="shrink-0 rounded p-1 text-muted-foreground opacity-0 transition group-hover:opacity-100 hover:bg-muted hover:text-foreground"
                  onclick={(ev) => handleOpenFolder(photo, ev)}
                  title={i18n.t('photoFolderBtn')}
                  aria-label={i18n.t('photoFolderBtn')}
                >
                  <FolderOpen size={12} />
                </button>
              </div>
            </div>
          {/each}
        </div>
      </section>
    {/each}
  </div>
{/if}
