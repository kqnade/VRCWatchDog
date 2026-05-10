<script lang="ts">
  import { convertFileSrc } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';
  import { ExternalLink, MapPin, Video as VideoIcon } from 'lucide-svelte';
  import { listRecentVideos } from '$lib/api/commands';
  import { i18n } from '$lib/i18n/use_t.svelte';
  import PageHeader from '$lib/ui/PageHeader.svelte';
  import Skeleton from '$lib/ui/Skeleton.svelte';
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

<PageHeader
  title={i18n.t('videosTitle')}
  description={isLoading
    ? i18n.t('loading')
    : i18n.t('photosCountFormat', { count: videos.length, max: PAGE_SIZE })}
/>

{#if loadError}
  <div class="mb-4 rounded-md border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive">
    {loadError}
  </div>
{/if}

{#if isLoading}
  <div class="space-y-2">
    {#each Array(8) as _, i (i)}
      <Skeleton class="h-20 w-full" />
    {/each}
  </div>
{:else if videos.length === 0 && !loadError}
  <p class="text-sm text-muted-foreground">{i18n.t('videosEmpty')}</p>
{:else}
  <ul class="space-y-2">
    {#each videos as video (video.id)}
      {@const thumbSrc = video.thumbnailPath ? convertFileSrc(video.thumbnailPath) : null}
      <li class="flex gap-3 rounded-lg border border-border bg-card p-3 transition-colors hover:border-primary/40">
        {#if thumbSrc}
          <img
            src={thumbSrc}
            alt={video.title ?? ''}
            class="h-16 w-28 shrink-0 rounded-md bg-muted object-cover"
            loading="lazy"
          />
        {:else}
          <div class="flex h-16 w-28 shrink-0 items-center justify-center rounded-md bg-muted text-muted-foreground">
            <VideoIcon size={20} />
          </div>
        {/if}
        <div class="min-w-0 flex-1">
          <div class="flex items-baseline justify-between gap-3">
            <span class="truncate text-sm font-medium" title={video.title ?? video.url}>
              {video.title ?? hostOf(video.url)}
            </span>
            <span class="shrink-0 font-mono text-xs text-muted-foreground">
              {formatTime(video.detectedUtc)}
            </span>
          </div>
          <a
            href={video.url}
            target="_blank"
            rel="noreferrer"
            class="mt-1 flex items-center gap-1 truncate font-mono text-xs text-muted-foreground hover:text-foreground hover:underline"
            title={video.url}
          >
            <ExternalLink size={10} class="shrink-0" />
            <span class="truncate">{video.url}</span>
          </a>
          {#if video.worldVisitId}
            <a
              href="/history?visit={video.worldVisitId}"
              class="mt-1 inline-flex items-center gap-1 font-mono text-xs text-muted-foreground hover:text-foreground hover:underline"
            >
              <MapPin size={10} />visit #{video.worldVisitId}
            </a>
          {/if}
        </div>
      </li>
    {/each}
  </ul>
{/if}
