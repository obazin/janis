// The five prism accents. Class maps are spelled out as literal strings so
// Tailwind's scanner sees every utility.

export type AccentColor = 'accent' | 'violet' | 'teal' | 'lime' | 'ember';

export const ACCENT_TEXT: Record<AccentColor, string> = {
    accent: 'text-accent',
    violet: 'text-violet',
    teal: 'text-teal',
    lime: 'text-lime',
    ember: 'text-ember',
};

export const ACCENT_STROKE: Record<AccentColor, string> = {
    accent: 'stroke-accent',
    violet: 'stroke-violet',
    teal: 'stroke-teal',
    lime: 'stroke-lime',
    ember: 'stroke-ember',
};
