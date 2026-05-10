<script lang="ts">
  import { open } from '@tauri-apps/plugin-dialog';
  import { onMount } from 'svelte';
  import { getSettings, saveSettings } from '$lib/api/commands';
  import { i18n } from '$lib/i18n/use_t.svelte';
  import type { Settings } from '$lib/api/types';

  // Phase 7.1.1: text input ベースの Settings form。
  // - log_directory / photo_directory はパスを直接入力 (folder picker は 7.1.2 で追加)。
  // - locale / theme は select。
  // - autostart / notification は checkbox (autostart plugin との同期は 7.1.3)。
  // - Save → saveSettings command。SettingsWriter actor が atomic + 100 並列保証 (plan §4)。
  let original = $state<Settings | null>(null);
  let form = $state<Settings | null>(null);
  let loadError = $state<string | null>(null);
  let saveError = $state<string | null>(null);
  let saveStatus = $state<'idle' | 'saving' | 'saved'>('idle');

  // dirty 判定 (未保存変更ありで save ボタン enable)
  const isDirty = $derived(
    form !== null && original !== null && JSON.stringify(form) !== JSON.stringify(original)
  );

  async function load() {
    try {
      const s = await getSettings();
      original = s;
      // 別 reference で持たないと bind が original を直接書き換える
      form = JSON.parse(JSON.stringify(s)) as Settings;
      loadError = null;
    } catch (e) {
      loadError = String(e);
    }
  }

  async function save() {
    if (!form) return;
    saveStatus = 'saving';
    saveError = null;
    try {
      await saveSettings(form);
      // locale が変わっていれば i18n に即時反映 (= 保存ボタン押下と同時に切替)
      i18n.setLocale(form.locale);
      original = JSON.parse(JSON.stringify(form)) as Settings;
      saveStatus = 'saved';
      setTimeout(() => {
        if (saveStatus === 'saved') saveStatus = 'idle';
      }, 2000);
    } catch (e) {
      saveError = String(e);
      saveStatus = 'idle';
    }
  }

  // tauri-plugin-dialog の `open({ directory: true })` でフォルダ選択ダイアログ。
  // キャンセル時は null。複数選択は無効 (multiple: false)。
  async function pickFolder(field: 'log_directory' | 'photo_directory' | 'thumbnail_cache_dir') {
    if (!form) return;
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        defaultPath: form[field] ?? undefined,
        title:
          field === 'log_directory'
            ? 'VRChat ログディレクトリを選択'
            : field === 'photo_directory'
              ? 'VRChat 写真ディレクトリを選択'
              : 'サムネキャッシュディレクトリを選択'
      });
      if (typeof selected === 'string' && selected.length > 0) {
        form[field] = selected;
      }
    } catch (e) {
      saveError = `フォルダ選択に失敗: ${e}`;
    }
  }

  function discard() {
    if (!original) return;
    form = JSON.parse(JSON.stringify(original)) as Settings;
    saveError = null;
    saveStatus = 'idle';
  }

  onMount(() => {
    void load();
  });
</script>

<main class="mx-auto min-h-screen max-w-2xl p-8">
  <header class="mb-6 flex items-baseline justify-between">
    <h1 class="text-2xl font-semibold">{i18n.t('settingsTitle')}</h1>
    <a href="/" class="text-sm text-muted-foreground hover:underline">{i18n.t('navHomeBack')}</a>
  </header>

  {#if loadError}
    <p class="mb-4 rounded border border-destructive bg-card px-3 py-2 text-sm text-destructive">
      {i18n.t('settingsLoadFailed')} {loadError}
    </p>
  {/if}

  {#if form}
    <!-- 入力 form。submit は preventDefault して save() を呼ぶ。 -->
    <form
      class="space-y-5"
      onsubmit={(e) => {
        e.preventDefault();
        void save();
      }}
    >
      <!-- log_directory -->
      <div class="space-y-1">
        <label for="log_directory" class="block text-sm font-medium">
          {i18n.t('settingsLogDir')}
        </label>
        <div class="flex gap-2">
          <input
            id="log_directory"
            type="text"
            class="flex-1 rounded border border-input bg-card px-3 py-2 font-mono text-sm focus:outline-none focus:ring-2 focus:ring-ring"
            placeholder="C:\Users\You\AppData\LocalLow\VRChat\VRChat"
            bind:value={
              () => form!.log_directory ?? '',
              (v: string) => (form!.log_directory = v.trim() === '' ? null : v)
            }
          />
          <button
            type="button"
            class="shrink-0 rounded border border-input px-3 py-2 text-sm hover:bg-card"
            onclick={() => pickFolder('log_directory')}
          >
            {i18n.t('settingsBrowse')}
          </button>
        </div>
      </div>

      <!-- photo_directory -->
      <div class="space-y-1">
        <label for="photo_directory" class="block text-sm font-medium">
          {i18n.t('settingsPhotoDir')}
        </label>
        <div class="flex gap-2">
          <input
            id="photo_directory"
            type="text"
            class="flex-1 rounded border border-input bg-card px-3 py-2 font-mono text-sm focus:outline-none focus:ring-2 focus:ring-ring"
            placeholder="C:\Users\You\Pictures\VRChat"
            bind:value={
              () => form!.photo_directory ?? '',
              (v: string) => (form!.photo_directory = v.trim() === '' ? null : v)
            }
          />
          <button
            type="button"
            class="shrink-0 rounded border border-input px-3 py-2 text-sm hover:bg-card"
            onclick={() => pickFolder('photo_directory')}
          >
            {i18n.t('settingsBrowse')}
          </button>
        </div>
      </div>

      <!-- locale -->
      <div class="space-y-1">
        <label for="locale" class="block text-sm font-medium">{i18n.t('settingsLocale')}</label>
        <select
          id="locale"
          class="rounded border border-input bg-card px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-ring"
          bind:value={form.locale}
        >
          <option value="ja">日本語 (ja)</option>
          <option value="en">English (en)</option>
        </select>
      </div>

      <!-- theme -->
      <div class="space-y-1">
        <label for="theme" class="block text-sm font-medium">{i18n.t('settingsTheme')}</label>
        <select
          id="theme"
          class="rounded border border-input bg-card px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-ring"
          bind:value={form.theme}
        >
          <option value="dark">{i18n.t('settingsThemeDark')}</option>
          <option value="light">{i18n.t('settingsThemeLight')}</option>
        </select>
      </div>

      <!-- toggles -->
      <div class="space-y-2">
        <label class="flex items-center gap-2 text-sm">
          <input type="checkbox" bind:checked={form.autostart_enabled} class="h-4 w-4" />
          {i18n.t('settingsAutostart')}
        </label>
        <label class="flex items-center gap-2 text-sm">
          <input type="checkbox" bind:checked={form.notification_enabled} class="h-4 w-4" />
          {i18n.t('settingsNotificationEnabled')}
        </label>
      </div>

      <!-- thumbnail_cache_dir (advanced, optional) -->
      <details class="rounded border border-input bg-card px-3 py-2">
        <summary class="cursor-pointer text-sm font-medium">詳細</summary>
        <div class="mt-3 space-y-1">
          <label for="thumbnail_cache_dir" class="block text-sm font-medium">
            {i18n.t('settingsThumbCache')}
          </label>
          <div class="flex gap-2">
            <input
              id="thumbnail_cache_dir"
              type="text"
              class="flex-1 rounded border border-input bg-card px-3 py-2 font-mono text-sm focus:outline-none focus:ring-2 focus:ring-ring"
              bind:value={
                () => form!.thumbnail_cache_dir ?? '',
                (v: string) => (form!.thumbnail_cache_dir = v.trim() === '' ? null : v)
              }
            />
            <button
              type="button"
              class="shrink-0 rounded border border-input px-3 py-2 text-sm hover:bg-card"
              onclick={() => pickFolder('thumbnail_cache_dir')}
            >
              {i18n.t('settingsBrowse')}
            </button>
          </div>
        </div>
      </details>

      <!-- 状態 + アクション -->
      <div class="flex items-center justify-between">
        <p class="text-sm">
          {#if saveError}
            <span class="text-destructive">{i18n.t('settingsSaveFailed')} {saveError}</span>
          {:else if saveStatus === 'saved'}
            <span class="text-success">{i18n.t('settingsSaved')}</span>
          {:else if saveStatus === 'saving'}
            <span class="opacity-60">{i18n.t('loading')}</span>
          {:else if isDirty}
            <span class="opacity-60">{i18n.t('settingsDirty')}</span>
          {/if}
        </p>
        <div class="flex gap-2">
          <button
            type="submit"
            disabled={!isDirty || saveStatus === 'saving'}
            class="rounded bg-foreground px-4 py-1.5 text-sm font-medium text-background hover:opacity-90 disabled:opacity-40"
          >
            {i18n.t('settingsSave')}
          </button>
        </div>
      </div>
    </form>
  {:else if !loadError}
    <p class="text-sm opacity-55">{i18n.t('loading')}</p>
  {/if}
</main>
