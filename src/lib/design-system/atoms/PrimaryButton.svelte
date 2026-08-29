<script lang="ts">
    import type { Snippet } from 'svelte';

    // Gradient CTA (Add music, Connect). Optional leading icon path.
    interface Props {
        children: Snippet;
        onclick?: () => void;
        icon?: string;
        variant?: 'gradient' | 'spotify';
    }

    let { children, onclick, icon, variant = 'gradient' }: Props = $props();
</script>

<button
    class="inline-flex items-center gap-2 cursor-pointer text-body-em font-bold text-on-accent px-4.5 py-2.75 transition-transform duration-fast hover:scale-[1.02]
    {variant === 'spotify'
        ? 'bg-spotify rounded-full font-extrabold px-7 py-3.5'
        : 'bg-cta-gradient rounded-btn'}"
    {onclick}
>
    {#if icon}
        <svg
            class="size-4.25"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2.4"
            stroke-linecap="round"
        >
            <path d={icon} />
        </svg>
    {/if}
    {@render children()}
</button>
