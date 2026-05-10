<script lang="ts">
  // Tailwind v4 entry を全 route に流し込む。`@tailwindcss/vite` が CSS を解決し、
  // SvelteKit が CSP-friendly な link rel="stylesheet" として埋める。
  import '../app.css';
  import { onMount } from 'svelte';
  import { getSelfPlayer } from '$lib/api/commands';
  import type { SelfPlayer } from '$lib/api/types';

  let { children } = $props();

  // Phase G: 直近 User Authenticated を全 page で表示する。layout なので
  // ページ遷移しても再 fetch しない (= 1 セッション内で安定)。
  let self = $state<SelfPlayer | null>(null);

  onMount(async () => {
    try {
      self = await getSelfPlayer();
    } catch {
      // self_player は無くてもアプリは動くので無視
      self = null;
    }
  });
</script>

{#if self?.displayName}
  <div
    class="pointer-events-none fixed right-3 top-3 z-50 rounded-full border bg-card/80 px-3 py-1 font-mono text-[11px] text-muted-foreground backdrop-blur"
    title={self.authenticatedUtc ?? ''}
  >
    @{self.displayName}
  </div>
{/if}

{@render children?.()}
