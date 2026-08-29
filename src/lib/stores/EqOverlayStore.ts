import { writable } from 'svelte/store';

// Open/closed state of the EQ bottom sheet. A bare writable: several screens
// and the mini player open it; the overlay itself closes it.
export const eqOpen = writable(false);
