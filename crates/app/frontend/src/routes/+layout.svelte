<script lang="ts">
  // Tailwind v4 entry を全 route に流し込む。`@tailwindcss/vite` が CSS を解決し、
  // SvelteKit が CSP-friendly な link rel="stylesheet" として埋める。
  import '../app.css';

  let { children } = $props();
</script>

{@render children?.()}
