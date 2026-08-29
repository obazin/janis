import type { TranslationKey } from '$lib/i18n/types';

// The built-in graphic-EQ presets (dB per band, low → high). `custom` is not
// a preset — it's the name the store reports once a band is hand-moved.

export const EQ_PRESETS = {
    flat: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    rock: [5, 4, 2, 0, -1, 0, 2, 3, 4, 4],
    pop: [-1, 1, 3, 4, 3, 1, 0, -1, -1, -1],
    jazz: [4, 3, 1, 2, -1, -1, 0, 1, 3, 4],
    classical: [4, 3, 2, 0, 0, 0, -1, -1, 2, 3],
    bassboost: [7, 6, 5, 3, 1, 0, 0, 0, 0, 0],
    vocal: [-2, -1, 0, 2, 4, 5, 4, 2, 0, -1],
    electronic: [5, 4, 1, 0, -2, 1, 0, 2, 4, 5],
} as const;

export type EqPresetName = keyof typeof EQ_PRESETS;

export const PRESET_ORDER: EqPresetName[] = [
    'flat',
    'rock',
    'pop',
    'jazz',
    'classical',
    'bassboost',
    'vocal',
    'electronic',
];

export const PRESET_LABEL_KEYS: Record<EqPresetName | 'custom', TranslationKey> = {
    flat: 'eq.preset.flat',
    rock: 'eq.preset.rock',
    pop: 'eq.preset.pop',
    jazz: 'eq.preset.jazz',
    classical: 'eq.preset.classical',
    bassboost: 'eq.preset.bassboost',
    vocal: 'eq.preset.vocal',
    electronic: 'eq.preset.electronic',
    custom: 'eq.preset.custom',
};
