// Phase C: theme + accent runtime applier。
// settings.theme ("dark" / "light" / "system") + settings.accent_color から
// `<html>` の class と `data-accent` 属性を更新する。
//
// `system` は `prefers-color-scheme` を MediaQueryList で監視する。

import type { Settings } from './api/types';

const ACCENT_PALETTE = [
  'violet',
  'blue',
  'teal',
  'green',
  'amber',
  'rose',
  'slate',
  'indigo',
] as const;
export type AccentColor = (typeof ACCENT_PALETTE)[number];
export const ACCENT_COLORS: readonly AccentColor[] = ACCENT_PALETTE;

export type ThemeMode = 'light' | 'dark' | 'system';
export const THEME_MODES: readonly ThemeMode[] = ['light', 'dark', 'system'] as const;

function isAccent(s: string): s is AccentColor {
  return (ACCENT_PALETTE as readonly string[]).includes(s);
}

function isThemeMode(s: string): s is ThemeMode {
  return s === 'light' || s === 'dark' || s === 'system';
}

let mediaQuery: MediaQueryList | null = null;
let mediaListener: ((e: MediaQueryListEvent) => void) | null = null;

function effectiveTheme(mode: ThemeMode): 'light' | 'dark' {
  if (mode === 'system') {
    if (typeof window === 'undefined') return 'dark';
    return window.matchMedia?.('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
  }
  return mode;
}

function setRootClasses(resolved: 'light' | 'dark', accent: AccentColor) {
  const root = document.documentElement;
  root.classList.toggle('dark', resolved === 'dark');
  root.classList.toggle('light', resolved === 'light');
  root.dataset.accent = accent;
}

/** settings から theme + accent を適用する。`system` 時は media listener を貼り直す。 */
export function applyTheme(settings: Pick<Settings, 'theme' | 'accent_color'>): void {
  if (typeof document === 'undefined') return;
  const mode: ThemeMode = isThemeMode(settings.theme) ? settings.theme : 'dark';
  const accent: AccentColor = isAccent(settings.accent_color) ? settings.accent_color : 'violet';
  setRootClasses(effectiveTheme(mode), accent);

  // detach previous listener
  if (mediaQuery && mediaListener) {
    mediaQuery.removeEventListener('change', mediaListener);
    mediaListener = null;
  }

  // system mode: listen for OS-level change
  if (mode === 'system' && typeof window !== 'undefined') {
    mediaQuery = window.matchMedia('(prefers-color-scheme: dark)');
    mediaListener = (e) => {
      const root = document.documentElement;
      root.classList.toggle('dark', e.matches);
      root.classList.toggle('light', !e.matches);
    };
    mediaQuery.addEventListener('change', mediaListener);
  }
}
