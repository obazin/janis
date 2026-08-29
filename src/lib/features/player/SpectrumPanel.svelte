<script lang="ts">
    import { playerStore } from './PlayerStore.svelte';
    import { eqStore } from '$lib/features/eq/EqStore.svelte';
    import { visualizer, visualPalette } from './visualizer';
    import { FREQ_LABELS } from './audioGraph';
    import { eqOpen } from '$lib/stores/EqOverlayStore';
    import { t } from '$lib/i18n/LanguageStore.svelte';
    import SectionLabel from '$lib/design-system/atoms/SectionLabel.svelte';
    import { ICONS } from '$lib/icons/paths';

    // The 10-band spectrum card on Now Playing: header (label + Open
    // Equalizer pill), live bars, frequency legend.

    let canvas = $state<HTMLCanvasElement | null>(null);

    function roundedRect(
        g: CanvasRenderingContext2D,
        x: number,
        y: number,
        w: number,
        h: number,
        r: number,
    ) {
        g.beginPath();
        g.moveTo(x + r, y);
        g.arcTo(x + w, y, x + w, y + h, r);
        g.arcTo(x + w, y + h, x, y + h, r);
        g.arcTo(x, y + h, x, y, r);
        g.arcTo(x, y, x + w, y, r);
        g.closePath();
    }

    function draw(now: number) {
        visualizer.tick(now, playerStore.analyser, playerStore.playing, eqStore.gains);
        if (!canvas) return;
        const dpr = window.devicePixelRatio || 1;
        const w = canvas.clientWidth;
        const h = canvas.clientHeight;
        if (w === 0 || h === 0) return;
        if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(h * dpr)) {
            canvas.width = Math.round(w * dpr);
            canvas.height = Math.round(h * dpr);
        }
        const g = canvas.getContext('2d');
        if (!g) return;
        g.setTransform(dpr, 0, 0, dpr, 0, 0);
        g.clearRect(0, 0, w, h);
        const colors = visualPalette();
        const n = visualizer.mags.length;
        const gap = w * 0.02;
        const bw = (w - gap * (n - 1)) / n;
        for (let i = 0; i < n; i++) {
            const x = i * (bw + gap);
            const bh = Math.max(3, visualizer.mags[i] * h);
            const radius = Math.min(6, bw / 2);
            g.fillStyle = colors.track;
            roundedRect(g, x, 0, bw, h, radius);
            g.fill();
            const grad = g.createLinearGradient(0, h, 0, h - bh);
            grad.addColorStop(0, colors.accent);
            grad.addColorStop(0.5, colors.violet);
            grad.addColorStop(1, colors.teal);
            g.fillStyle = grad;
            roundedRect(g, x, h - bh, bw, bh, radius);
            g.fill();
        }
    }

    $effect(() => {
        let raf = 0;
        const loop = (now: number) => {
            draw(now);
            raf = requestAnimationFrame(loop);
        };
        raf = requestAnimationFrame(loop);
        return () => cancelAnimationFrame(raf);
    });
</script>

<div class="bg-well-soft border border-border rounded-panel px-5 py-4.5">
    <div class="flex justify-between items-center mb-3">
        <SectionLabel>{t('now.spectrum')}</SectionLabel>
        <button
            class="flex items-center gap-1.75 cursor-pointer text-body font-bold text-accent px-3.25 py-1.5 bg-accent/12 rounded-full border border-accent/30 transition-colors duration-fast hover:bg-accent/20"
            onclick={() => eqOpen.set(true)}
        >
            <svg
                class="size-3.75"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
            >
                <path d={ICONS.equalizer} />
            </svg>
            {t('now.openEq')}
        </button>
    </div>
    <div class="h-22">
        <canvas bind:this={canvas} class="size-full"></canvas>
    </div>
    <div class="grid grid-cols-10 mt-2">
        {#each FREQ_LABELS as label (label)}
            <div class="text-center font-mono text-micro text-text-faint">{label}</div>
        {/each}
    </div>
</div>
