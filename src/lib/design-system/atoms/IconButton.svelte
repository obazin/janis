<script lang="ts">
    import type { IconButtonIdentifier } from './IconButtonIdentifier';

    // Icon-only button rendering central path data (`$lib/icons/paths`).
    // `mode` picks stroke glyphs (shuffle, repeat) vs filled glyphs
    // (prev/next). Requires a registry identifier (see IconButtonIdentifier).
    interface Props {
        identifier: IconButtonIdentifier;
        d: string;
        label: string;
        onclick?: () => void;
        mode?: 'stroke' | 'fill';
        size?: 'sm' | 'md';
        active?: boolean;
        tone?: 'default' | 'secondary' | 'muted';
    }

    let {
        identifier,
        d,
        label,
        onclick,
        mode = 'stroke',
        size = 'sm',
        active = false,
        tone = 'default',
    }: Props = $props();

    const TONE = {
        default: 'text-text',
        secondary: 'text-text-secondary',
        muted: 'text-text-muted',
    } as const;

    const SIZE = { sm: 'size-5', md: 'size-6.5' } as const;
</script>

<button
    data-identifier={identifier}
    aria-label={label}
    aria-pressed={active}
    class="cursor-pointer transition-colors duration-fast hover:text-text {active
        ? 'text-accent'
        : TONE[tone]}"
    {onclick}
>
    {#if mode === 'stroke'}
        <svg
            class={SIZE[size]}
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
        >
            <path {d} />
        </svg>
    {:else}
        <svg class={SIZE[size]} viewBox="0 0 24 24" fill="currentColor">
            <path {d} />
        </svg>
    {/if}
</button>
