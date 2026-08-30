<script lang="ts">
    // Album/cover art tile. Shows real cover art when a URL is given,
    // otherwise a deterministic prism gradient derived from `seed`, with an
    // optional initials monogram. Caller sizes it (`class`) — the tile fills
    // its box.
    //
    // When art was expected but its source failed — a dead image URL (`onerror`
    // below) or an upstream fetch the caller flags via `unavailable` — the tile
    // falls back to the gradient AND shows a muted "unavailable" mark, so a
    // failed load reads differently from art that simply doesn't exist.
    //
    // `class` must not set `position`: this root is `relative` so the art and
    // the gloss can sit on top of each other, and Tailwind emits `.relative`
    // after `.absolute`, so an `absolute` passed in would silently lose and
    // leave the tile with no height. Position a wrapper instead.
    import { ICONS } from '$lib/icons/paths';

    interface Props {
        seed: string;
        coverUrl?: string | null;
        initials?: string | null;
        gradIndex?: number;
        /** Caller knows art was expected but its source failed to load. */
        unavailable?: boolean;
        /** Fired when the `<img>` itself fails to load (dead URL / bad data). */
        onImageError?: () => void;
        class?: string;
    }

    let {
        seed,
        coverUrl = null,
        initials = null,
        gradIndex,
        unavailable = false,
        onImageError,
        class: extra = '',
    }: Props = $props();

    // Ordered pairs of prism-accent CSS variables; index picked by seed hash.
    const GRADIENTS: [string, string][] = [
        ['--color-accent', '--color-violet'],
        ['--color-teal', '--color-violet'],
        ['--color-ember', '--color-accent'],
        ['--color-lime', '--color-teal'],
        ['--color-violet', '--color-accent'],
        ['--color-teal', '--color-lime'],
    ];

    function hash(s: string): number {
        let h = 0;
        for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) | 0;
        return Math.abs(h);
    }

    const pair = $derived(GRADIENTS[(gradIndex ?? hash(seed)) % GRADIENTS.length]);

    // Remember which URL failed rather than a bare flag, so a fresh `coverUrl`
    // is automatically a fresh attempt (the same dead URL stays failed).
    let failedUrl = $state<string | null>(null);
    const imgFailed = $derived(coverUrl !== null && coverUrl === failedUrl);
    const failed = $derived(unavailable || imgFailed);

    function handleImgError() {
        failedUrl = coverUrl;
        onImageError?.();
    }
</script>

<div class="relative overflow-hidden art-gloss {extra}">
    {#if coverUrl && !imgFailed}
        <img
            src={coverUrl}
            alt=""
            class="absolute inset-0 size-full object-cover"
            onerror={handleImgError}
        />
    {:else}
        <div
            class="absolute inset-0"
            style="background: linear-gradient(140deg, var({pair[0]}), var({pair[1]}))"
        ></div>
        {#if failed}
            <div class="absolute inset-0 flex items-center justify-center text-text-muted/70">
                <svg
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1.4"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    class="size-1/3 max-w-8"
                    aria-hidden="true"
                >
                    <path d={ICONS.imageOff} />
                </svg>
            </div>
        {:else if initials}
            <div
                class="absolute bottom-2 left-2.5 font-display font-black text-heading-lg leading-hero opacity-85"
            >
                {initials}
            </div>
        {/if}
    {/if}
</div>
