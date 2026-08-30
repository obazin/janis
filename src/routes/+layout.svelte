<script lang="ts">
    import '../app.css';
    import type { Snippet } from 'svelte';
    import { browser } from '$app/environment';
    import { invoke } from '@tauri-apps/api/core';
    import { getCurrentWebview } from '@tauri-apps/api/webview';
    import type { Preferences } from '$lib/models/Preferences';
    import { languageStore, t } from '$lib/i18n/LanguageStore.svelte';
    import { eqStore } from '$lib/features/eq/EqStore.svelte';
    import { playerStore } from '$lib/features/player/PlayerStore.svelte';
    import { preferencesStore } from '$lib/features/settings/PreferencesStore.svelte';
    import { libraryStore } from '$lib/features/library/LibraryStore.svelte';
    import { STATIONS } from '$lib/features/radio/stations';
    import { navigationStore, navigateTo, type Screen } from '$lib/stores/NavigationStore.svelte';
    import AppTitlebar from '$lib/screens/AppTitlebar.svelte';
    import SidebarNav from '$lib/design-system/organisms/SidebarNav.svelte';
    import MiniPlayer from '$lib/features/player/MiniPlayer.svelte';
    import EqualizerOverlay from '$lib/features/eq/EqualizerOverlay.svelte';
    import Toaster from '$lib/design-system/organisms/Toaster.svelte';
    import { ICONS } from '$lib/icons/paths';

    interface Props {
        children: Snippet;
    }

    let { children }: Props = $props();

    // Persistent stores boot here, before anything renders — a screen's
    // onMount always sees hydrated state. `browser` gate keeps prerender a
    // no-op.
    const bootPromise = browser ? boot() : new Promise<void>(() => {});

    async function boot() {
        // Attach to the engine first: it has been running since setup, so a
        // reloaded webview needs its state replayed before anything renders.
        await playerStore.connect();
        const prefs = await invoke<Preferences>('get_preferences');
        languageStore.init(prefs.language);
        eqStore.init(prefs.eqGains, prefs.eqPreset);
        playerStore.initVolume(prefs.volume);
        preferencesStore.init(prefs);
        await libraryStore.init();
        // The engine replays its queue and station as bare ids; the objects
        // they name live in the library and the radio catalog. This layer is
        // the one allowed to bridge features, so it hands the player its
        // lookups — after the library is hydrated, so a reloaded webview can
        // resolve the replayed queue immediately.
        playerStore.registerLookups({
            trackById: (id) => libraryStore.trackById(id),
            stationById: (id) => STATIONS.find((station) => station.id === id),
        });
        // OS drag-and-drop delivers real paths through Tauri, not the DOM.
        // App-lifetime listener — never unlistened (rule: task lifecycles are
        // independent of UI lifecycles). A registration failure loses the
        // drop shortcut, never the app: boot must not reject over it.
        try {
            await getCurrentWebview().onDragDropEvent((event) => {
                if (event.payload.type === 'drop') {
                    void libraryStore.importPaths(event.payload.paths);
                }
            });
        } catch (err) {
            console.error('drag-drop listener failed:', err);
        }
    }

    const sections = $derived([
        {
            label: t('nav.section.playing'),
            items: [{ id: 'now-playing', label: t('nav.nowPlaying'), d: ICONS.nowPlaying }],
        },
        {
            label: t('nav.section.library'),
            items: [
                { id: 'library', label: t('nav.library'), d: ICONS.library },
                { id: 'discover', label: t('nav.discover'), d: ICONS.discover },
            ],
        },
        {
            label: t('nav.section.sources'),
            items: [
                { id: 'radio', label: t('nav.radio'), d: ICONS.radio },
                { id: 'local', label: t('nav.local'), d: ICONS.localFiles },
                { id: 'spotify', label: t('nav.spotify'), d: ICONS.spotify },
            ],
        },
    ]);
</script>

{#await bootPromise then}
    <div class="relative h-full flex flex-col overflow-hidden">
        <AppTitlebar />
        <div class="flex-1 flex min-h-0">
            <SidebarNav
                {sections}
                footerItem={{ id: 'settings', label: t('nav.settings'), d: ICONS.equalizer }}
                activeId={navigationStore.current}
                onItemClick={(id) => navigateTo(id as Screen)}
            />
            <main class="flex-1 min-w-0 overflow-y-auto mscroll">
                {@render children()}
            </main>
        </div>
        <MiniPlayer />
        <EqualizerOverlay />
        <Toaster />
    </div>
{/await}
