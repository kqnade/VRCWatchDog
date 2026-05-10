<script lang="ts">
  import { convertFileSrc } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';
  import { listRecentPhotos, openPhoto, openPhotoFolder } from '$lib/api/commands';
  import type { PhotoRecord } from '$lib/api/types';

  // Phase 6.2.2: photo_grid 仮版。
  // - サムネ生成 (Phase 6.3 thumb_writer) はまだ無いので、box にファイル名 + 撮影時刻 +
  //   world_visit_id バッジを置くだけのプレースホルダ表示。
  // - クリック → open_photo command で OS の関連付けされたアプリで開く。
  // - 「フォルダで開く」ボタン → open_photo_folder で Explorer を開く。
  let photos = $state<PhotoRecord[]>([]);
  let loadError = $state<string | null>(null);
  let isLoading = $state(true);

  const PAGE_SIZE = 100;

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

  async function handleOpen(photo: PhotoRecord) {
    try {
      await openPhoto(photo.filePath);
    } catch (e) {
      // backend の path traversal 防御等で reject される可能性
      loadError = `写真を開けませんでした: ${e}`;
    }
  }

  async function handleOpenFolder(photo: PhotoRecord, ev: MouseEvent | KeyboardEvent) {
    ev.stopPropagation();
    try {
      await openPhotoFolder(photo.filePath);
    } catch (e) {
      loadError = `フォルダを開けませんでした: ${e}`;
    }
  }

  // div role="button" で keyboard 操作 (Enter/Space) もサポート
  function onCardKey(photo: PhotoRecord, ev: KeyboardEvent) {
    if (ev.key === 'Enter' || ev.key === ' ') {
      ev.preventDefault();
      void handleOpen(photo);
    }
  }

  onMount(() => {
    void load();
  });
</script>

<main class="mx-auto min-h-screen max-w-6xl p-8">
  <header class="mb-6 flex items-baseline justify-between">
    <div>
      <h1 class="text-2xl font-semibold">Photos</h1>
      <p class="mt-1 text-sm opacity-60">
        {#if isLoading}
          読込中…
        {:else}
          直近 {photos.length} 件 (最大 {PAGE_SIZE} 件)
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

  {#if !isLoading && photos.length === 0 && !loadError}
    <p class="text-sm opacity-55">
      写真がまだ取り込まれていません。Settings で
      <code class="rounded bg-muted px-1.5 py-0.5 font-mono">photo_directory</code>
      を設定し、PhotoScanner が走るのを待ってください。
    </p>
  {/if}

  <div
    class="grid gap-3"
    style="grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));"
  >
    {#each photos as photo (photo.id)}
      <!-- card は button にできない (内部に「フォルダを開く」 button があるため
           HTML 仕様で nested button 不可)。div + role="button" + keydown で代替。 -->
      <div
        role="button"
        tabindex="0"
        class="group flex cursor-pointer flex-col rounded-md border bg-card p-3 text-left transition hover:border-muted-foreground focus:outline-none focus:ring-2 focus:ring-ring"
        onclick={() => handleOpen(photo)}
        onkeydown={(ev) => onCardKey(photo, ev)}
        title="クリック / Enter で開く"
      >
        <!-- サムネ表示。thumb_writer (Phase 6.3) が thumbPath を埋めたら asset:// に。
             tauri.conf.json の assetProtocol.scope は thumbs/** だけ通している。 -->
        <div class="mb-2 flex aspect-video items-center justify-center overflow-hidden rounded bg-muted text-xs opacity-50">
          {#if photo.thumbPath}
            <img
              src={convertFileSrc(photo.thumbPath)}
              alt={photo.fileName}
              class="h-full w-full object-cover"
              loading="lazy"
            />
          {:else}
            no thumb yet
          {/if}
        </div>
        <p class="truncate font-mono text-xs" title={photo.fileName}>{photo.fileName}</p>
        <p class="mt-1 text-xs opacity-60">{photo.takenNaiveLocal}</p>
        <div class="mt-2 flex items-center justify-between gap-2 text-xs">
          {#if photo.worldVisitId}
            <a
              href="/history?visit={photo.worldVisitId}"
              class="truncate rounded bg-muted px-1.5 py-0.5 text-muted-foreground hover:underline"
              title={photo.worldName ?? `visit #${photo.worldVisitId}`}
              onclick={(ev) => ev.stopPropagation()}
            >
              {photo.worldName ?? `visit #${photo.worldVisitId}`}
            </a>
          {:else}
            <span class="opacity-40">no visit</span>
          {/if}
          <button
            type="button"
            class="shrink-0 opacity-0 transition group-hover:opacity-100 hover:underline"
            onclick={(ev) => handleOpenFolder(photo, ev)}
            title="フォルダを Explorer で開く"
          >
            📁 folder
          </button>
        </div>
      </div>
    {/each}
  </div>
</main>
