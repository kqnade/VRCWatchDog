<script lang="ts">
  import { open } from '@tauri-apps/plugin-dialog';
  import { onMount } from 'svelte';
  import {
    Bell,
    Check,
    FolderOpen,
    Languages,
    Monitor,
    Moon,
    Palette,
    Power,
    Sun,
  } from 'lucide-svelte';
  import { getSettings, saveSettings } from '$lib/api/commands';
  import { i18n } from '$lib/i18n/use_t.svelte';
  import { applyTheme, ACCENT_COLORS, type AccentColor, type ThemeMode } from '$lib/theme.svelte';
  import Button from '$lib/ui/Button.svelte';
  import Card from '$lib/ui/Card.svelte';
  import Input from '$lib/ui/Input.svelte';
  import PageHeader from '$lib/ui/PageHeader.svelte';
  import type { Settings } from '$lib/api/types';

  let original = $state<Settings | null>(null);
  let form = $state<Settings | null>(null);
  let loadError = $state<string | null>(null);
  let saveError = $state<string | null>(null);
  let saveStatus = $state<'idle' | 'saving' | 'saved'>('idle');

  const isDirty = $derived(
    form !== null && original !== null && JSON.stringify(form) !== JSON.stringify(original)
  );

  const themeOptions: Array<{ value: ThemeMode; labelKey: 'settingsThemeLight' | 'settingsThemeDark' | 'settingsThemeSystem'; icon: typeof Sun }> = [
    { value: 'light', labelKey: 'settingsThemeLight', icon: Sun },
    { value: 'dark', labelKey: 'settingsThemeDark', icon: Moon },
    { value: 'system', labelKey: 'settingsThemeSystem', icon: Monitor },
  ];

  const accentSwatches: Record<AccentColor, string> = {
    violet: 'hsl(270 76% 60%)',
    blue: 'hsl(217 91% 60%)',
    teal: 'hsl(173 80% 40%)',
    green: 'hsl(142 71% 45%)',
    amber: 'hsl(38 92% 50%)',
    rose: 'hsl(350 89% 60%)',
    slate: 'hsl(215 16% 47%)',
    indigo: 'hsl(239 84% 67%)',
  };

  async function load() {
    try {
      const s = await getSettings();
      original = s;
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
      i18n.setLocale(form.locale);
      applyTheme(form);
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

  /** preview: dirty かつ未保存でも live で見えるよう、UI 触った瞬間にテーマ反映する。 */
  function previewTheme(theme: string, accent: string) {
    applyTheme({ theme, accent_color: accent });
  }

  function setTheme(t: ThemeMode) {
    if (!form) return;
    form.theme = t;
    previewTheme(t, form.accent_color);
  }
  function setAccent(a: AccentColor) {
    if (!form) return;
    form.accent_color = a;
    previewTheme(form.theme, a);
  }

  async function pickFolder(field: 'log_directory' | 'photo_directory' | 'thumbnail_cache_dir') {
    if (!form) return;
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        defaultPath: form[field] ?? undefined,
      });
      if (typeof selected === 'string' && selected.length > 0) {
        form[field] = selected;
      }
    } catch (e) {
      saveError = `${e}`;
    }
  }

  onMount(() => {
    void load();
  });
</script>

<PageHeader title={i18n.t('settingsTitle')} />

{#if loadError}
  <div class="mb-4 rounded-md border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive">
    {i18n.t('settingsLoadFailed')} {loadError}
  </div>
{/if}

{#if form}
  <form
    class="space-y-5"
    onsubmit={(e) => {
      e.preventDefault();
      void save();
    }}
  >
    <!-- Appearance -->
    <Card title={i18n.t('settingsAppearance')} description={i18n.t('settingsAppearanceDesc')}>
      <div class="space-y-4">
        <!-- theme mode -->
        <div>
          <div class="mb-2 flex items-center gap-2 text-sm font-medium">
            <Palette size={14} />{i18n.t('settingsTheme')}
          </div>
          <div class="grid grid-cols-3 gap-2">
            {#each themeOptions as opt (opt.value)}
              {@const Icon = opt.icon}
              {@const active = form.theme === opt.value}
              <button
                type="button"
                onclick={() => setTheme(opt.value)}
                class="flex items-center justify-center gap-2 rounded-md border px-3 py-2 text-sm transition {active
                  ? 'border-primary bg-primary/10 text-foreground'
                  : 'border-border bg-card text-muted-foreground hover:border-primary/50'}"
              >
                <Icon size={14} />{i18n.t(opt.labelKey)}
              </button>
            {/each}
          </div>
        </div>

        <!-- accent color -->
        <div>
          <div class="mb-2 flex items-center gap-2 text-sm font-medium">
            <Palette size={14} />{i18n.t('settingsAccentColor')}
          </div>
          <div class="flex flex-wrap gap-2">
            {#each ACCENT_COLORS as color (color)}
              {@const active = form.accent_color === color}
              <button
                type="button"
                onclick={() => setAccent(color)}
                class="relative flex h-9 w-9 items-center justify-center rounded-md border-2 transition {active
                  ? 'border-foreground'
                  : 'border-transparent hover:border-border'}"
                style="background: {accentSwatches[color]}"
                aria-label={color}
                title={color}
              >
                {#if active}<Check size={14} class="text-white drop-shadow" />{/if}
              </button>
            {/each}
          </div>
        </div>

        <!-- locale -->
        <div>
          <div class="mb-2 flex items-center gap-2 text-sm font-medium">
            <Languages size={14} />{i18n.t('settingsLocale')}
          </div>
          <div class="grid grid-cols-2 gap-2">
            {#each [{ v: 'ja', l: '日本語' }, { v: 'en', l: 'English' }] as opt (opt.v)}
              {@const active = form.locale === opt.v}
              <button
                type="button"
                onclick={() => form && (form.locale = opt.v)}
                class="rounded-md border px-3 py-2 text-sm transition {active
                  ? 'border-primary bg-primary/10 text-foreground'
                  : 'border-border bg-card text-muted-foreground hover:border-primary/50'}"
              >
                {opt.l}
              </button>
            {/each}
          </div>
        </div>
      </div>
    </Card>

    <!-- Directories -->
    <Card title={i18n.t('settingsPaths')} description={i18n.t('settingsPathsDesc')}>
      <div class="space-y-4">
        <div class="space-y-1.5">
          <label for="log_directory" class="block text-sm font-medium">
            {i18n.t('settingsLogDir')}
          </label>
          <div class="flex gap-2">
            <Input
              id="log_directory"
              type="text"
              placeholder="C:\Users\You\AppData\LocalLow\VRChat\VRChat"
              class="font-mono text-xs"
              bind:value={
                () => form!.log_directory ?? '',
                (v: string) => (form!.log_directory = v.trim() === '' ? null : v)
              }
            />
            <Button variant="outline" size="default" onclick={() => pickFolder('log_directory')}>
              <FolderOpen size={14} />{i18n.t('settingsBrowse')}
            </Button>
          </div>
        </div>

        <div class="space-y-1.5">
          <label for="photo_directory" class="block text-sm font-medium">
            {i18n.t('settingsPhotoDir')}
          </label>
          <div class="flex gap-2">
            <Input
              id="photo_directory"
              type="text"
              placeholder="C:\Users\You\Pictures\VRChat"
              class="font-mono text-xs"
              bind:value={
                () => form!.photo_directory ?? '',
                (v: string) => (form!.photo_directory = v.trim() === '' ? null : v)
              }
            />
            <Button variant="outline" size="default" onclick={() => pickFolder('photo_directory')}>
              <FolderOpen size={14} />{i18n.t('settingsBrowse')}
            </Button>
          </div>
        </div>

        <div class="space-y-1.5">
          <label for="thumbnail_cache_dir" class="block text-sm font-medium">
            {i18n.t('settingsThumbCache')}
          </label>
          <div class="flex gap-2">
            <Input
              id="thumbnail_cache_dir"
              type="text"
              class="font-mono text-xs"
              bind:value={
                () => form!.thumbnail_cache_dir ?? '',
                (v: string) => (form!.thumbnail_cache_dir = v.trim() === '' ? null : v)
              }
            />
            <Button variant="outline" size="default" onclick={() => pickFolder('thumbnail_cache_dir')}>
              <FolderOpen size={14} />{i18n.t('settingsBrowse')}
            </Button>
          </div>
        </div>
      </div>
    </Card>

    <!-- Behavior -->
    <Card title={i18n.t('settingsBehavior')} description={i18n.t('settingsBehaviorDesc')}>
      <div class="space-y-3">
        <label class="flex cursor-pointer items-center justify-between rounded-md border border-border px-3 py-2.5 text-sm hover:bg-muted/40">
          <span class="flex items-center gap-2">
            <Power size={14} />{i18n.t('settingsAutostart')}
          </span>
          <input type="checkbox" bind:checked={form.autostart_enabled} class="h-4 w-4 accent-primary" />
        </label>
        <label class="flex cursor-pointer items-center justify-between rounded-md border border-border px-3 py-2.5 text-sm hover:bg-muted/40">
          <span class="flex items-center gap-2">
            <Bell size={14} />{i18n.t('settingsNotificationEnabled')}
          </span>
          <input type="checkbox" bind:checked={form.notification_enabled} class="h-4 w-4 accent-primary" />
        </label>
      </div>
    </Card>

    <!-- Save bar -->
    <div class="sticky bottom-0 -mx-8 -mb-8 flex items-center justify-between border-t border-border bg-background/95 px-8 py-3 backdrop-blur">
      <p class="text-sm">
        {#if saveError}
          <span class="text-destructive">{i18n.t('settingsSaveFailed')} {saveError}</span>
        {:else if saveStatus === 'saved'}
          <span class="text-success">{i18n.t('settingsSaved')}</span>
        {:else if saveStatus === 'saving'}
          <span class="text-muted-foreground">{i18n.t('loading')}</span>
        {:else if isDirty}
          <span class="text-muted-foreground">{i18n.t('settingsDirty')}</span>
        {/if}
      </p>
      <Button type="submit" disabled={!isDirty || saveStatus === 'saving'}>
        {i18n.t('settingsSave')}
      </Button>
    </div>
  </form>
{:else if !loadError}
  <p class="text-sm text-muted-foreground">{i18n.t('loading')}</p>
{/if}
