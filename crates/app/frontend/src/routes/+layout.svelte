<script lang="ts">
  // Tailwind v4 entry を全 route に流し込む。`@tailwindcss/vite` が CSS を解決し、
  // SvelteKit が CSP-friendly な link rel="stylesheet" として埋める。
  import '../app.css';
  import { onMount } from 'svelte';
  import { getSelfPlayer, getSettings } from '$lib/api/commands';
  import { i18n } from '$lib/i18n/use_t.svelte';
  import type { SelfPlayer } from '$lib/api/types';

  let { children } = $props();

  // Phase G: 直近 User Authenticated を全 page で表示する。layout なので
  // ページ遷移しても再 fetch しない (= 1 セッション内で安定)。
  // Phase F: settings.locale を i18n に流し込む (起動時 1 回。settings 画面で save
  // した直後は settings 画面側が i18n.setLocale を即時呼ぶ)。
  let self = $state<SelfPlayer | null>(null);

  onMount(async () => {
    try {
      const s = await getSettings();
      i18n.setLocale(s.locale);
    } catch {
      // settings load 失敗時は default ja のまま
    }
    try {
      self = await getSelfPlayer();
    } catch {
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
