<script lang="ts">
    import { STATIONS, GENRE_FILTERS } from '$lib/features/radio/stations';
    import StationCard from '$lib/features/radio/StationCard.svelte';
    import { playerStore } from '$lib/features/player/PlayerStore.svelte';
    import { searchQuery } from '$lib/stores/SearchStore';
    import Chip from '$lib/design-system/atoms/Chip.svelte';
    import Eyebrow from '$lib/design-system/atoms/Eyebrow.svelte';
    import { t } from '$lib/i18n/LanguageStore.svelte';
    import type { TranslationKey } from '$lib/i18n/types';

    let genre = $state<TranslationKey>('radio.genre.all');

    // "All" first, then genres by their label in the active language.
    const genreChips = $derived([
        GENRE_FILTERS[0],
        ...GENRE_FILTERS.slice(1).sort((a, b) => t(a).localeCompare(t(b))),
    ]);

    const query = $derived($searchQuery.trim().toLowerCase());
    const stations = $derived(
        STATIONS.filter(
            (s) =>
                (genre === 'radio.genre.all' || s.genreKey === genre) &&
                (!query ||
                    s.name.toLowerCase().includes(query) ||
                    t(s.genreKey).toLowerCase().includes(query)),
        ),
    );
</script>

<div class="px-11 pt-9 pb-10 animate-float-up">
    <Eyebrow color="ember" class="mb-2">{t('radio.eyebrow')}</Eyebrow>
    <h1 class="font-display font-black text-display tracking-tight m-0 mb-5">
        {t('radio.title')}
    </h1>
    <div class="flex gap-2 flex-wrap mb-6.5">
        {#each genreChips as key (key)}
            <Chip label={t(key)} active={genre === key} onclick={() => (genre = key)} />
        {/each}
    </div>
    <div class="grid grid-cols-[repeat(auto-fill,minmax(280px,1fr))] gap-4">
        {#each stations as station (station.id)}
            <StationCard
                {station}
                active={playerStore.station?.id === station.id}
                connecting={playerStore.connecting && playerStore.station?.id === station.id}
                onclick={() => playerStore.playStation(station)}
            />
        {/each}
    </div>
</div>
