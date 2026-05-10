<script lang="ts" module>
  export type BadgeVariant =
    | 'default'
    | 'secondary'
    | 'outline'
    | 'success'
    | 'warning'
    | 'destructive';

  const VARIANTS: Record<BadgeVariant, string> = {
    default: 'bg-primary/15 text-primary border-transparent',
    secondary: 'bg-muted text-muted-foreground border-transparent',
    outline: 'bg-transparent text-foreground border-border',
    success: 'bg-success/15 text-success border-transparent',
    warning: 'bg-warning/15 text-warning border-transparent',
    destructive: 'bg-destructive/15 text-destructive border-transparent',
  };
</script>

<script lang="ts">
  import type { Snippet } from 'svelte';

  type Props = {
    variant?: BadgeVariant;
    class?: string;
    children?: Snippet;
  };

  let { variant = 'default', class: className = '', children }: Props = $props();
</script>

<span
  class="inline-flex items-center rounded-md border px-2 py-0.5 text-[11px] font-medium {VARIANTS[
    variant
  ]} {className}"
>
  {@render children?.()}
</span>
