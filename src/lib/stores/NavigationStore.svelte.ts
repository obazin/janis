import { page } from '$app/state';
import { goto } from '$app/navigation';

export const SCREENS = [
    'now-playing',
    'library',
    'discover',
    'radio',
    'local',
    'spotify',
    'settings',
] as const;
export type Screen = (typeof SCREENS)[number];

// Not "state" we own — a typed reactive view of SvelteKit's router, plus the
// single navigation chokepoint. Never call `goto()` directly elsewhere.
class NavigationStore {
    readonly current = $derived.by<Screen>(() => {
        const path = page.url.pathname.replace(/^\//, '');
        return (SCREENS as readonly string[]).includes(path) ? (path as Screen) : 'now-playing';
    });
}

export const navigationStore = new NavigationStore();

export function navigateTo(screen: Screen) {
    goto(`/${screen}`);
}
