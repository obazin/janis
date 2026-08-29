<script lang="ts">
    import type { Snippet } from 'svelte';
    import { ACCENT_TEXT, type AccentColor } from './accent';

    // The small colored all-caps line above a screen title. Each screen keys
    // it to its identity accent.
    interface Props {
        color: AccentColor;
        children: Snippet;
        class?: string;
    }

    let { color, children, class: extra = '' }: Props = $props();
</script>

<div class="text-caption font-bold tracking-eyebrow uppercase {ACCENT_TEXT[color]} {extra}">
    {@render children()}
</div>
