<script lang="ts">
    import { playerStore } from './PlayerStore.svelte';
    import { t } from '$lib/i18n/LanguageStore.svelte';

    // Click/drag volume rail with the pink→teal fill.

    let rail: HTMLDivElement;
    let dragging = $state(false);

    function apply(event: PointerEvent) {
        const rect = rail.getBoundingClientRect();
        playerStore.setVolume((event.clientX - rect.left) / rect.width);
    }

    function onPointerDown(event: PointerEvent) {
        rail.setPointerCapture(event.pointerId);
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

<div
    bind:this={rail}
    role="slider"
    tabindex="0"
    aria-label={t('player.volume')}
    aria-valuemin={0}
    aria-valuemax={100}
    aria-valuenow={Math.round(playerStore.volume * 100)}
    class="relative w-27.5 h-1.5 bg-rail rounded-full cursor-pointer touch-none"
    onpointerdown={onPointerDown}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
>
    <div
        class="absolute left-0 top-0 h-full bg-volume-gradient rounded-full"
        style="width: {playerStore.volume * 100}%"
    ></div>
</div>
