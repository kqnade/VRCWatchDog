import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

// SPA-style static build. Tauri は vite build の出力を ../dist 相当として食う。
// fallback: 'index.html' で SvelteKit の SPA モードが有効になる (+layout.ts で
// prerender=true / ssr=false にしているため)。
/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess(),
  kit: {
    adapter: adapter({
      pages: 'build',
      assets: 'build',
      fallback: 'index.html',
      precompress: false,
      strict: true
    })
  }
};

export default config;
