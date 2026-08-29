<script lang="ts">
    import { playerStore } from './PlayerStore.svelte';
    import { eqStore } from '$lib/features/eq/EqStore.svelte';
    import { visualizer, visualPalette } from './visualizer';
    import { t } from '$lib/i18n/LanguageStore.svelte';

    // Live waveform (time-domain oscilloscope). `big` — the Now Playing
    // strip: progress wash + playhead dot, click to seek. `mini` — the
    // bottom-bar strip: line only.
    interface Props {
        variant: 'big' | 'mini';
    }

    let { variant }: Props = $props();

    let canvas = $state<HTMLCanvasElement | null>(null);
    const big = $derived(variant === 'big');

    function fit(c: HTMLCanvasElement) {
        const dpr = window.devicePixelRatio || 1;
        const w = c.clientWidth;
        const h = c.clientHeight;
        if (w === 0 || h === 0) return null;
        if (c.width !== Math.round(w * dpr) || c.height !== Math.round(h * dpr)) {
            c.width = Math.round(w * dpr);
            c.height = Math.round(h * dpr);
        }
        const g = c.getContext('2d');
        if (!g) return null;
        g.setTransform(dpr, 0, 0, dpr, 0, 0);
        g.clearRect(0, 0, w, h);
        return { g, w, h };
    }

    function draw(now: number) {
        visualizer.tick(now, playerStore.analyser, playerStore.playing, eqStore.gains);
        if (!canvas) return;
        const f = fit(canvas);
        if (!f) return;
        const { g, w, h } = f;
        const colors = visualPalette();
        const mid = h / 2;
        const amp = h * 0.42;
        const progress = playerStore.liveProgress();
        const wave = visualizer.wave;
        const n = wave.length;

        if (big && progress > 0) {
            g.fillStyle = colors.accent;
            g.globalAlpha = 0.08;
            g.fillRect(0, 0, w * progress, h);
            g.globalAlpha = 1;
        }

        const grad = g.createLinearGradient(0, 0, w, 0);
        grad.addColorStop(0, colors.accent);
        grad.addColorStop(0.5, colors.violet);
        grad.addColorStop(1, colors.teal);
        g.lineWidth = big ? 2.5 : 2;
        g.strokeStyle = grad;
        g.shadowColor = colors.accent;
        g.shadowBlur = big ? 14 : 6;
        g.beginPath();
        for (let i = 0; i < n; i++) {
            const x = (i / (n - 1)) * w;
            const y = mid + wave[i] * amp;
            if (i === 0) g.moveTo(x, y);
            else g.lineTo(x, y);
        }
        g.stroke();
        g.shadowBlur = 0;

        if (big && progress > 0) {
            const px = w * progress;
            g.fillStyle = colors.playhead;
            g.shadowColor = colors.playhead;
            g.shadowBlur = 12;
            g.beginPath();
            g.arc(px, mid + wave[Math.floor(progress * (n - 1))] * amp, 4, 0, 7);
            g.fill();
            g.shadowBlur = 0;
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

    function onSeekClick(event: MouseEvent) {
        if (!big) return;
        const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
        playerStore.seekTo((event.clientX - rect.left) / rect.width);
    }
</script>

{#if big}
    <button
        class="relative block w-full h-full cursor-pointer"
        aria-label={t('player.seek')}
        onclick={onSeekClick}
    >
        <canvas bind:this={canvas} class="absolute inset-0 size-full"></canvas>
    </button>
{:else}
    <canvas bind:this={canvas} class="size-full"></canvas>
{/if}
