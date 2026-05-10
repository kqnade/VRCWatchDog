<script lang="ts">
  import { convertFileSrc } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';
  import { listRecentVideos } from '$lib/api/commands';
  import { i18n } from '$lib/i18n/use_t.svelte';
  import type { Video } from '$lib/api/types';

  let videos = $state<Video[]>([]);
  let loadError = $state<string | null>(null);
  let isLoading = $state(true);

  const PAGE_SIZE = 200;

  function formatTime(iso: string): string {
    const d = new Date(iso);
    const yyyy = d.getFullYear();
    const mm = String(d.getMonth() + 1).padStart(2, '0');
    const dd = String(d.getDate()).padStart(2, '0');
    const hh = String(d.getHours()).padStart(2, '0');
    const mi = String(d.getMinutes()).padStart(2, '0');
    return `${yyyy}-${mm}-${dd} ${hh}:${mi}`;
  }

  // ホスト名を URL から取り出す表示用 (youtube.com / twitch.tv / nicovideo.jp 等)
  function hostOf(url: string): string {
    try {
      return new URL(url).hostname.replace(/^www\./, '');
    } catch {
      return url;
    }
  }

  async function load() {
    isLoading = true;
    try {
      videos = await listRecentVideos(PAGE_SIZE);
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
      <h1 class="text-2xl font-semibold">{i18n.t('videosTitle')}</h1>
      <p class="mt-1 text-sm opacity-60">
        {#if isLoading}
          {i18n.t('loading')}
        {:else}
          {i18n.t('photosCountFormat', { count: videos.length, max: PAGE_SIZE })}
        {/if}
      </p>
    </div>
    <a href="/" class="text-sm text-muted-foreground hover:underline">{i18n.t('navHomeBack')}</a>
  </header>

  {#if loadError}
    <p class="mb-4 rounded border border-destructive bg-card px-3 py-2 text-sm text-destructive">
      {loadError}
    </p>
  {/if}

  {#if !isLoading && videos.length === 0 && !loadError}
    <p class="text-sm opacity-55">{i18n.t('videosEmpty')}</p>
  {/if}

  <ul class="space-y-2">
    {#each videos as video (video.id)}
      {@const thumbSrc = video.thumbnailPath ? convertFileSrc(video.thumbnailPath) : null}
      <li class="flex gap-3 rounded-md border bg-card p-3">
        {#if thumbSrc}
          <img
            src={thumbSrc}
            alt={video.title ?? ''}
            class="h-16 w-28 shrink-0 rounded bg-muted object-cover"
            loading="lazy"
          />
        {:else}
          <div
            class="flex h-16 w-28 shrink-0 items-center justify-center rounded bg-muted text-[10px] opacity-50"
          >
            {video.title ? '...' : 'no thumb'}
          </div>
        {/if}
        <div class="min-w-0 flex-1">
          <div class="flex items-baseline justify-between gap-3">
            <span class="truncate text-sm font-medium" title={video.title ?? video.url}>
              {video.title ?? hostOf(video.url)}
            </span>
            <span class="shrink-0 font-mono text-xs opacity-60">
              {formatTime(video.detectedUtc)}
            </span>
          </div>
          <a
            href={video.url}
            target="_blank"
            rel="noreferrer"
            class="mt-1 block truncate font-mono text-xs text-muted-foreground hover:underline"
            title={video.url}
          >
            {video.url}
          </a>
          {#if video.worldVisitId}
            <a
              href="/history?visit={video.worldVisitId}"
              class="mt-1 inline-block font-mono text-xs text-muted-foreground hover:underline"
            >
              visit #{video.worldVisitId}
            </a>
          {/if}
        </div>
      </li>
    {/each}
  </ul>
</main>
