// Svelte 5 runes ($state / $derived) は `.svelte` か `.svelte.ts` でしか使えないので、
// store.ts (plain TS) と分離してこちらに reactive layer を置く。
//
// 使い方:
//   <script>
//     import { i18n } from '$lib/i18n/use_t.svelte';
//   </script>
//   <h1>{i18n.t('photosTitle')}</h1>
//
// `i18n.t()` は内部で `i18n.locale` を読むので、setLocale() で locale が変わると
// `$state` 経由で全 caller が rerender される。

import {
  getLocale,
  setLocale as setLocaleImpl,
  subscribeLocale,
} from './store';
import { type Locale, type Messages, messages } from './messages';

class I18nReactive {
  /** rune-tracked locale。初期値は store の現在値。 */
  locale: Locale = $state(getLocale());

  constructor() {
    // store 側の変化を $state にミラー。listener は session 中ずっと残す
    // (Svelte ライフサイクル外なので unsubscribe しない)。
    subscribeLocale((loc) => {
      this.locale = loc;
    });
  }

  /** `i18n.locale` 経由で読むので Svelte の dependency graph に乗る。 */
  t(key: keyof Messages, params?: Record<string, string | number>): string {
    let s = messages[this.locale][key];
    if (params) {
      for (const [k, v] of Object.entries(params)) {
        s = s.replace(new RegExp(`\\{${k}\\}`, 'g'), String(v));
      }
    }
    return s;
  }

  setLocale(input: string | null | undefined): void {
    setLocaleImpl(input);
  }
}

/** module-level singleton。複数 import しても同じ instance。 */
export const i18n = new I18nReactive();
