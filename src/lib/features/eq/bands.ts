// The shape of the graphic EQ: ten bands, ±12 dB.
//
// The filters themselves run in Rust, so these are only what the frontend
// needs to label and bound the sliders. `audio/params.rs` holds the matching
// centre frequencies; the two lists must stay in step or the sliders lie
// about what they move.

export const CENTER_FREQS = [32, 64, 125, 250, 500, 1000, 2000, 4000, 8000, 16000] as const;

export const FREQ_LABELS = [
    '32',
    '64',
    '125',
    '250',
    '500',
    '1k',
    '2k',
    '4k',
    '8k',
    '16k',
] as const;

export const EQ_BAND_COUNT = 10;

/** Symmetric: sliders run from −12 dB to +12 dB. */
export const EQ_GAIN_RANGE = 12;
