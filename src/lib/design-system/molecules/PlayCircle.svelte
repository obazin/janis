<script lang="ts">
    import type { IconButtonIdentifier } from '../atoms/IconButtonIdentifier';
    import { ICONS } from '$lib/icons/paths';

    // Circular play/pause button. `hero` — the 60px gradient pulse-glow
    // button on Now Playing; `mini` — the 42px light button in the bottom
    // bar.
    interface Props {
        identifier: IconButtonIdentifier;
        playing: boolean;
        onclick: () => void;
        variant?: 'hero' | 'mini';
        label: string;
    }

    let { identifier, playing, onclick, variant = 'hero', label }: Props = $props();
</script>

<button
    data-identifier={identifier}
    aria-label={label}
    class="flex items-center justify-center rounded-full cursor-pointer flex-none
    {variant === 'hero' ? 'size-15 bg-play-gradient animate-pulse-glow' : 'size-10.5 bg-text'}"
    {onclick}
>
    <svg
        class="{variant === 'hero' ? 'size-6.5' : 'size-5'} text-on-accent"
        viewBox="0 0 24 24"
        fill="currentColor"
    >
        <path d={playing ? ICONS.pause : ICONS.play} />
    </svg>
</button>
