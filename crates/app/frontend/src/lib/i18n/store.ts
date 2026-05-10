// Phase F: locale を 1 個の global module-level state に持つ。
// settings load 時 / settings save 後に layout から `setLocale()` を呼ぶ。
// `t(key, params?)` は messages を引いて placeholder を埋める。

import {
  type Locale,
  type Messages,
  messages,
  normalizeLocale,
} from './messages';

// Svelte 5 runes を module スコープで使うと top-level reactive にならない (`$state` は
// `.svelte` / `.svelte.ts` 専用)。ここは module 変数 + subscriber callback で代替する。
// 数 component が listen するだけなので listener Set で十分。
let currentLocale: Locale = 'ja';
const listeners = new Set<(loc: Locale) => void>();

export function getLocale(): Locale {
  return currentLocale;
}

export function setLocale(input: string | null | undefined): void {
  const next = normalizeLocale(input);
  if (next === currentLocale) return;
  currentLocale = next;
  for (const fn of listeners) fn(next);
}

/** locale 変化を購読する。component の onMount で呼び、unmount で必ず unsubscribe。 */
export function subscribeLocale(fn: (loc: Locale) => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

/** 現在の locale で 1 文字列を引く。`{name}` プレースホルダは params で置換。 */
export function t(key: keyof Messages, params?: Record<string, string | number>): string {
  let s = messages[currentLocale][key];
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      s = s.replace(new RegExp(`\\{${k}\\}`, 'g'), String(v));
    }
  }
  return s;
}
