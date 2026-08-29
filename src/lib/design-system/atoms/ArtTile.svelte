<script lang="ts">
    // Album/cover art tile. Shows real cover art when a URL is given,
    // otherwise a deterministic prism gradient derived from `seed`, with an
    // optional initials monogram. Caller sizes it (`class`) — the tile fills
    // its box.
    //
    // `class` must not set `position`: this root is `relative` so the art and
    // the gloss can sit on top of each other, and Tailwind emits `.relative`
    // after `.absolute`, so an `absolute` passed in would silently lose and
    // leave the tile with no height. Position a wrapper instead.
    interface Props {
        seed: string;
        coverUrl?: string | null;
        initials?: string | null;
        gradIndex?: number;
        class?: string;
    }

    let { seed, coverUrl = null, initials = null, gradIndex, class: extra = '' }: Props = $props();

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
</script>

<div class="relative overflow-hidden art-gloss {extra}">
    {#if coverUrl}
        <img src={coverUrl} alt="" class="absolute inset-0 size-full object-cover" />
    {:else}
        <div
            class="absolute inset-0"
            style="background: linear-gradient(140deg, var({pair[0]}), var({pair[1]}))"
        ></div>
        {#if initials}
            <div
                class="absolute bottom-2 left-2.5 font-display font-black text-heading-lg leading-hero opacity-85"
            >
                {initials}
            </div>
        {/if}
    {/if}
</div>
