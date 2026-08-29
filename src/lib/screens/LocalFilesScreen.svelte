<script lang="ts">
    import { libraryStore } from '$lib/features/library/LibraryStore.svelte';
    import { playerStore } from '$lib/features/player/PlayerStore.svelte';
    import { searchQuery } from '$lib/stores/SearchStore';
    import type { Track } from '$lib/models/Track';
    import LocalRow from '$lib/features/library/LocalRow.svelte';
    import Eyebrow from '$lib/design-system/atoms/Eyebrow.svelte';
    import SectionLabel from '$lib/design-system/atoms/SectionLabel.svelte';
    import GhostButton from '$lib/design-system/atoms/GhostButton.svelte';
    import { ICONS } from '$lib/icons/paths';
    import { t } from '$lib/i18n/LanguageStore.svelte';

    const query = $derived($searchQuery.trim().toLowerCase());
    const files = $derived(
        query
            ? libraryStore.tracks.filter(
                  (tr) =>
                      tr.title.toLowerCase().includes(query) ||
                      (tr.artist ?? '').toLowerCase().includes(query),
              )
            : [...libraryStore.tracks],
    );

    function play(track: Track) {
        playerStore.playQueue([...files], files.indexOf(track));
    }
</script>

<div class="px-11 pt-9 pb-10 animate-float-up">
    <Eyebrow color="lime" class="mb-2">{t('local.eyebrow')}</Eyebrow>
    <h1 class="font-display font-black text-display tracking-tight m-0 mb-6">
        {t('local.title')}
    </h1>

    <button
        class="w-full border-2 border-dashed border-lime/40 rounded-art px-5 py-11 text-center cursor-pointer bg-lime/3 transition-colors duration-base hover:bg-lime/8"
        onclick={() => libraryStore.addFiles()}
    >
        <svg
            class="size-11.5 text-lime inline-block"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.6"
            stroke-linecap="round"
            stroke-linejoin="round"
        >
            <path d={ICONS.upload} />
        </svg>
        <div class="font-display font-extrabold text-heading-md mt-3.5">
            {t('local.drop.title')}
        </div>
        <div class="text-body-em text-text-muted mt-1">{t('local.drop.desc')}</div>
        <div class="text-body text-lime mt-3.5 font-semibold">{t('local.drop.note')}</div>
    </button>

    <div class="flex justify-between items-center mt-7 mb-2">
        <SectionLabel>{t('local.scanned', { count: files.length })}</SectionLabel>
        <div class="flex items-center gap-2.5">
            {#if libraryStore.lastReport}
                <span class="text-caption text-text-muted">
                    {t('local.scanReport', {
                        added: libraryStore.lastReport.added,
                        updated: libraryStore.lastReport.updated,
                        skipped: libraryStore.lastReport.skipped,
                    })}
                </span>
            {/if}
            <GhostButton icon={ICONS.plus} onclick={() => libraryStore.addFolder()}>
                {t('local.addFolder')}
            </GhostButton>
            <GhostButton icon={ICONS.refresh} onclick={() => libraryStore.rescan()}>
                {t('local.rescan')}
            </GhostButton>
        </div>
    </div>

    {#if files.length}
        <div class="rounded-card overflow-hidden border border-border">
            <div
                class="flex gap-4 px-4 py-2.5 text-label font-bold tracking-section uppercase text-text-faint bg-panel"
            >
                <div class="flex-1">{t('local.col.title')}</div>
                <div class="w-37.5">{t('local.col.artist')}</div>
                <div class="w-20">{t('local.col.format')}</div>
                <div class="w-17.5 text-right">{t('local.col.time')}</div>
            </div>
            {#each files as track, i (track.id)}
                <LocalRow {track} first={i === 0} onclick={() => play(track)} />
            {/each}
        </div>
    {/if}

    {#if libraryStore.folders.length}
        <SectionLabel class="mt-7 mb-2">{t('local.folders')}</SectionLabel>
        <div class="rounded-card overflow-hidden border border-border">
            {#each libraryStore.folders as folder, i (folder.id)}
                <div
                    class="flex items-center gap-4 px-4 py-2.75 text-body-em
                    {i > 0 ? 'border-t border-divider' : ''}"
                >
                    <svg
                        class="size-4.25 text-text-muted flex-none"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="1.9"
                    >
                        <path d={ICONS.folder} />
                    </svg>
                    <div class="flex-1 min-w-0 truncate text-text-secondary">{folder.path}</div>
                    <div class="text-caption text-text-muted">
                        {t('library.tracks', { count: folder.trackCount })}
                    </div>
                    <GhostButton onclick={() => libraryStore.removeFolder(folder.id)}>
                        {t('local.removeFolder')}
                    </GhostButton>
                </div>
            {/each}
        </div>
    {/if}
</div>
