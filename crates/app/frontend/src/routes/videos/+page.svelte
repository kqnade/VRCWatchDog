<script lang="ts">
  import { onMount } from 'svelte';
  import { listRecentVideos } from '$lib/api/commands';
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
      <h1 class="text-2xl font-semibold">Videos</h1>
      <p class="mt-1 text-sm opacity-60">
        {#if isLoading}
          読込中…
        {:else}
          直近 {videos.length} 件 (最大 {PAGE_SIZE} 件)
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

  {#if !isLoading && videos.length === 0 && !loadError}
    <p class="text-sm opacity-55">動画ログはまだ記録されていません。</p>
  {/if}

  <ul class="space-y-2">
    {#each videos as video (video.id)}
      <li class="rounded-md border bg-card p-3">
        <div class="flex items-baseline justify-between gap-3">
          <span class="truncate text-sm font-medium" title={video.title ?? video.url}>
            {video.title ?? hostOf(video.url)}
          </span>
          <span class="shrink-0 font-mono text-xs opacity-60">{formatTime(video.detectedUtc)}</span>
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
          <p class="mt-1 text-xs opacity-55">visit #{video.worldVisitId}</p>
        {/if}
      </li>
    {/each}
  </ul>
</main>
