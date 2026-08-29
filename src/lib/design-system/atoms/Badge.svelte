<script lang="ts">
    import type { Snippet } from 'svelte';
    import type { AccentColor } from './accent';

    // Small pill label. `outline` — hairline uppercase tag (OPEN SOURCE);
    // `soft` — neutral filled chip (Hi-Res · FLAC 24/96); `tint` — accent-
    // tinted chip (Lossless), colored via `tone`.
    interface Props {
        children: Snippet;
        variant?: 'outline' | 'soft' | 'tint';
        tone?: AccentColor;
    }

    let { children, variant = 'soft', tone = 'lime' }: Props = $props();

    const TINT: Record<AccentColor, string> = {
        accent: 'text-accent bg-accent/12',
        violet: 'text-violet bg-violet/12',
        teal: 'text-teal bg-teal/12',
        lime: 'text-lime bg-lime/12',
        ember: 'text-ember bg-ember/12',
    };
</script>

{#if variant === 'outline'}
    <span
        class="text-label font-semibold tracking-pill uppercase text-text-muted px-1.75 py-0.5 border border-border-emphasis rounded-full"
    >
        {@render children()}
    </span>
{:else if variant === 'tint'}
    <span class="text-caption font-semibold px-3 py-1.5 rounded-full {TINT[tone]}">
        {@render children()}
    </span>
{:else}
    <span class="text-caption font-semibold text-text-secondary px-3 py-1.5 bg-chip rounded-full">
        {@render children()}
    </span>
{/if}
