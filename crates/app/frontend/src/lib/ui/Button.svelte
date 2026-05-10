<script lang="ts" module>
  // shadcn-svelte 風 Button。primary は accent token を使う。
  // variant: default (= primary, accent), secondary, outline, ghost, destructive
  // size: default, sm, lg, icon
  export type ButtonVariant = 'default' | 'secondary' | 'outline' | 'ghost' | 'destructive';
  export type ButtonSize = 'default' | 'sm' | 'lg' | 'icon';

  const VARIANTS: Record<ButtonVariant, string> = {
    default:
      'bg-primary text-primary-foreground hover:bg-primary/90 shadow-sm',
    secondary:
      'bg-secondary text-secondary-foreground hover:bg-secondary/80',
    outline:
      'border border-border bg-transparent hover:bg-muted hover:text-foreground',
    ghost:
      'bg-transparent hover:bg-muted hover:text-foreground',
    destructive:
      'bg-destructive text-destructive-foreground hover:bg-destructive/90 shadow-sm',
  };
  const SIZES: Record<ButtonSize, string> = {
    default: 'h-9 px-4 py-2 text-sm',
    sm: 'h-8 px-3 text-xs',
    lg: 'h-10 px-6 text-base',
    icon: 'h-9 w-9 p-0',
  };

  export function buttonClass(variant: ButtonVariant = 'default', size: ButtonSize = 'default'): string {
    const base =
      'inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50';
    return `${base} ${VARIANTS[variant]} ${SIZES[size]}`;
  }
</script>

<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { HTMLButtonAttributes } from 'svelte/elements';

  type Props = HTMLButtonAttributes & {
    variant?: ButtonVariant;
    size?: ButtonSize;
    children?: Snippet;
  };

  let {
    variant = 'default',
    size = 'default',
    type = 'button',
    class: className = '',
    children,
    ...rest
  }: Props = $props();
</script>

<button {type} class="{buttonClass(variant, size)} {className}" {...rest}>
  {@render children?.()}
</button>
