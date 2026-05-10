<script lang="ts">
  import type { HTMLInputAttributes } from 'svelte/elements';

  type Props = Omit<HTMLInputAttributes, 'value'> & {
    class?: string;
    value?: string;
  };

  let { class: className = '', value = $bindable(''), ...rest }: Props = $props();
</script>

<input
  class="flex h-9 w-full rounded-md border border-border bg-background px-3 py-1 text-sm shadow-sm transition-colors placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50 {className}"
  bind:value
  {...rest}
/>
