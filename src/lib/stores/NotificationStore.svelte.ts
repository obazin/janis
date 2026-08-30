import type { TranslationKey } from '$lib/i18n/types';

// App-wide, ephemeral notifications — the one place a failure becomes visible
// to the user instead of a silent console line. A rune class with methods as
// the single write chokepoint (it owns the dismiss timers); read through the
// `items` getter.
//
// Messages travel as a translation key + params, not a resolved string, so a
// toast re-renders in the active language and this store stays i18n-agnostic.

export interface Notification {
    id: number;
    messageKey: TranslationKey;
    params?: Record<string, string | number>;
}

const DEFAULT_TIMEOUT_MS = 7000;

class NotificationStore {
    #items = $state<Notification[]>([]);
    #seq = 0;
    // Dedup key → live notification id, so a storm collapses into one toast.
    #keyed = new Map<string, number>();

    get items(): Notification[] {
        return this.#items;
    }

    /**
     * Raises an error toast. `dedupeKey` collapses a burst into a single
     * message — a disconnected NAS failing hundreds of covers shows one toast,
     * not hundreds. `timeoutMs <= 0` keeps it until dismissed.
     */
    error(
        messageKey: TranslationKey,
        opts: {
            params?: Record<string, string | number>;
            dedupeKey?: string;
            timeoutMs?: number;
        } = {},
    ) {
        if (opts.dedupeKey && this.#keyed.has(opts.dedupeKey)) return;
        const id = ++this.#seq;
        this.#items = [...this.#items, { id, messageKey, params: opts.params }];
        if (opts.dedupeKey) this.#keyed.set(opts.dedupeKey, id);
        const timeout = opts.timeoutMs ?? DEFAULT_TIMEOUT_MS;
        if (timeout > 0) setTimeout(() => this.dismiss(id), timeout);
    }

    dismiss(id: number) {
        this.#items = this.#items.filter((n) => n.id !== id);
        for (const [key, value] of this.#keyed) if (value === id) this.#keyed.delete(key);
    }
}

export const notificationStore = new NotificationStore();
