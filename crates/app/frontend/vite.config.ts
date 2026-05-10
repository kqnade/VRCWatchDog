import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

// Tauri は dev で 5173 固定を期待する (tauri.conf.json の devUrl と一致させる)。
// strictPort: true で被ったら fail-fast。clearScreen: false は Tauri からの
// インライン log を消さないため。
export default defineConfig({
  plugins: [sveltekit()],
  server: {
    port: 5173,
    strictPort: true,
    host: '127.0.0.1'
  },
  clearScreen: false
});
