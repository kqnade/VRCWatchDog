<script lang="ts">
  import type { Snippet } from 'svelte';

  type Props = {
    title?: string;
    description?: string;
    class?: string;
    header?: Snippet;
    footer?: Snippet;
    children?: Snippet;
  };

  let {
    title,
    description,
    class: className = '',
    header,
    footer,
    children,
  }: Props = $props();
</script>

<div class="rounded-lg border border-border bg-card text-card-foreground shadow-sm {className}">
  {#if header}
    <div class="border-b border-border px-5 py-3">{@render header()}</div>
  {:else if title || description}
    <div class="px-5 pb-3 pt-4">
      {#if title}<h2 class="text-base font-semibold leading-none">{title}</h2>{/if}
      {#if description}<p class="mt-1.5 text-xs text-muted-foreground">{description}</p>{/if}
    </div>
  {/if}
  <div class="px-5 py-4">{@render children?.()}</div>
  {#if footer}
    <div class="border-t border-border px-5 py-3">{@render footer()}</div>
  {/if}
</div>
