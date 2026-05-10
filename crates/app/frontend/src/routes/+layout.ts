// Tauri は WebView 内 SPA として動作する (Node/SSR は無い)。
// - ssr=false: load 関数も含め全て client 実行。Tauri の invoke() が SSR で
//   解決できず壊れるのを防ぐ。
// - prerender=true: vite build で全 route を index.html にプリレンダ。
export const ssr = false;
export const prerender = true;
