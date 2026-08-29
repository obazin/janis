<script lang="ts">
    import type { Snippet } from 'svelte';

    interface Props {
        children: Snippet;
        class?: string;
    }

    let { children, class: extra = '' }: Props = $props();
</script>

<div class="text-label font-bold tracking-section uppercase text-text-faint {extra}">
    {@render children()}
</div>
