<script lang="ts">
    import { EQ_GAIN_RANGE } from '$lib/features/player/audioGraph';

    // One vertical EQ band: value readout, draggable column, frequency
    // label. Controlled — caller owns the value and receives drags.
    interface Props {
        value: number;
        freqLabel: string;
        onInput: (value: number) => void;
    }

    let { value, freqLabel, onInput }: Props = $props();

    let column: HTMLDivElement;
    let dragging = $state(false);

    const fraction = $derived((value + EQ_GAIN_RANGE) / (EQ_GAIN_RANGE * 2));
    const topPct = $derived((1 - fraction) * 100);
    const fillTop = $derived(value >= 0 ? topPct : 50);
    const fillHeight = $derived(Math.abs(topPct - 50));

    function apply(event: PointerEvent) {
        const rect = column.getBoundingClientRect();
        const t = Math.min(1, Math.max(0, (event.clientY - rect.top) / rect.height));
        onInput((1 - t) * EQ_GAIN_RANGE * 2 - EQ_GAIN_RANGE);
    }

    function onPointerDown(event: PointerEvent) {
        column.setPointerCapture(event.pointerId);
        dragging = true;
        apply(event);
    }

    function onPointerMove(event: PointerEvent) {
        if (dragging) apply(event);
    }

    function onPointerUp() {
        dragging = false;
    }
</script>

<div class="flex-1 flex flex-col items-center gap-2.5">
    <div
        class="font-mono text-label font-bold
        {value > 0 ? 'text-lime' : value < 0 ? 'text-ember' : 'text-text-muted'}"
    >
        {value > 0 ? '+' : ''}{value}
    </div>
    <div
        bind:this={column}
        role="slider"
        tabindex="0"
        aria-label={freqLabel}
        aria-valuemin={-EQ_GAIN_RANGE}
        aria-valuemax={EQ_GAIN_RANGE}
        aria-valuenow={value}
        class="relative w-full max-w-8.5 h-47.5 bg-well-deep rounded-full cursor-ns-resize overflow-hidden touch-none"
        onpointerdown={onPointerDown}
        onpointermove={onPointerMove}
        onpointerup={onPointerUp}
    >
        <div class="absolute left-1/2 top-1/2 w-0.5 h-px bg-rail -translate-x-1/2"></div>
        <div
            class="absolute left-0 right-0 bg-eq-gradient opacity-55"
            style="top: {fillTop}%; height: {fillHeight}%"
        ></div>
        <div
            class="absolute left-1/2 w-6.5 h-3 rounded-md bg-text shadow-md ring-2 ring-accent/50 -translate-x-1/2 -translate-y-1/2"
            style="top: {topPct}%"
        ></div>
    </div>
    <div class="font-mono text-label text-text-muted">{freqLabel}</div>
</div>
