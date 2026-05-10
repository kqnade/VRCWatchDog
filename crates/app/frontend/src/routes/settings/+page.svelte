<script lang="ts">
  import { onMount } from 'svelte';
  import { getSettings, saveSettings } from '$lib/api/commands';
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
      original = JSON.parse(JSON.stringify(form)) as Settings;
      saveStatus = 'saved';
      // 数秒で 'idle' に戻す
      setTimeout(() => {
        if (saveStatus === 'saved') saveStatus = 'idle';
      }, 2000);
    } catch (e) {
      saveError = String(e);
      saveStatus = 'idle';
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
    <h1 class="text-2xl font-semibold">Settings</h1>
    <a href="/" class="text-sm text-muted-foreground hover:underline">← Home</a>
  </header>

  {#if loadError}
    <p class="mb-4 rounded border border-destructive bg-card px-3 py-2 text-sm text-destructive">
      設定の取得に失敗しました: {loadError}
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
          Log directory
          <span class="ml-1 text-xs opacity-55">(空なら VRChat 標準パスを自動検出)</span>
        </label>
        <input
          id="log_directory"
          type="text"
          class="w-full rounded border border-input bg-card px-3 py-2 font-mono text-sm focus:outline-none focus:ring-2 focus:ring-ring"
          placeholder="C:\Users\You\AppData\LocalLow\VRChat\VRChat"
          bind:value={
            () => form!.log_directory ?? '',
            (v: string) => (form!.log_directory = v.trim() === '' ? null : v)
          }
        />
      </div>

      <!-- photo_directory -->
      <div class="space-y-1">
        <label for="photo_directory" class="block text-sm font-medium">
          Photo directory
          <span class="ml-1 text-xs opacity-55">(必須: 設定するまで写真スキャンは動かない)</span>
        </label>
        <input
          id="photo_directory"
          type="text"
          class="w-full rounded border border-input bg-card px-3 py-2 font-mono text-sm focus:outline-none focus:ring-2 focus:ring-ring"
          placeholder="C:\Users\You\Pictures\VRChat"
          bind:value={
            () => form!.photo_directory ?? '',
            (v: string) => (form!.photo_directory = v.trim() === '' ? null : v)
          }
        />
      </div>

      <!-- locale -->
      <div class="space-y-1">
        <label for="locale" class="block text-sm font-medium">Locale</label>
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
        <label for="theme" class="block text-sm font-medium">Theme</label>
        <select
          id="theme"
          class="rounded border border-input bg-card px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-ring"
          bind:value={form.theme}
        >
          <option value="dark">Dark</option>
          <option value="light">Light</option>
        </select>
      </div>

      <!-- toggles -->
      <div class="space-y-2">
        <label class="flex items-center gap-2 text-sm">
          <input type="checkbox" bind:checked={form.autostart_enabled} class="h-4 w-4" />
          OS 起動時に自動で開始する
          <span class="text-xs opacity-55">(Phase 7.1.3 で実 plugin と同期)</span>
        </label>
        <label class="flex items-center gap-2 text-sm">
          <input type="checkbox" bind:checked={form.notification_enabled} class="h-4 w-4" />
          通知を受け取る
        </label>
      </div>

      <!-- thumbnail_cache_dir (advanced, optional) -->
      <details class="rounded border border-input bg-card px-3 py-2">
        <summary class="cursor-pointer text-sm font-medium">詳細</summary>
        <div class="mt-3 space-y-1">
          <label for="thumbnail_cache_dir" class="block text-sm font-medium">
            Thumbnail cache directory
            <span class="ml-1 text-xs opacity-55">(空なら %LOCALAPPDATA% 下のデフォルト)</span>
          </label>
          <input
            id="thumbnail_cache_dir"
            type="text"
            class="w-full rounded border border-input bg-card px-3 py-2 font-mono text-sm focus:outline-none focus:ring-2 focus:ring-ring"
            bind:value={
              () => form!.thumbnail_cache_dir ?? '',
              (v: string) => (form!.thumbnail_cache_dir = v.trim() === '' ? null : v)
            }
          />
        </div>
      </details>

      <!-- 状態 + アクション -->
      <div class="flex items-center justify-between">
        <p class="text-sm">
          {#if saveError}
            <span class="text-destructive">保存失敗: {saveError}</span>
          {:else if saveStatus === 'saved'}
            <span class="text-success">保存しました</span>
          {:else if saveStatus === 'saving'}
            <span class="opacity-60">保存中…</span>
          {:else if isDirty}
            <span class="opacity-60">未保存の変更があります</span>
          {:else}
            <span class="opacity-40">変更なし</span>
          {/if}
        </p>
        <div class="flex gap-2">
          <button
            type="button"
            disabled={!isDirty || saveStatus === 'saving'}
            onclick={discard}
            class="rounded border border-input px-3 py-1.5 text-sm hover:bg-card disabled:opacity-40"
          >
            破棄
          </button>
          <button
            type="submit"
            disabled={!isDirty || saveStatus === 'saving'}
            class="rounded bg-foreground px-4 py-1.5 text-sm font-medium text-background hover:opacity-90 disabled:opacity-40"
          >
            保存
          </button>
        </div>
      </div>
    </form>
  {:else if !loadError}
    <p class="text-sm opacity-55">読込中…</p>
  {/if}
</main>
