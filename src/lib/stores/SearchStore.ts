import { writable } from 'svelte/store';

// The titlebar search field. A bare writable (messaging, not durable state):
// the titlebar writes, list screens subscribe and filter.
export const searchQuery = writable('');
