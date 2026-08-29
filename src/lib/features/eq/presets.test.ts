import { describe, expect, it } from 'vitest';
import { EQ_PRESETS, PRESET_ORDER, PRESET_LABEL_KEYS } from './presets';
import { EQ_BAND_COUNT, EQ_GAIN_RANGE } from './bands';

describe('EQ presets', () => {
    it('every preset covers all bands within the gain range', () => {
        for (const [name, gains] of Object.entries(EQ_PRESETS)) {
            expect(gains, name).toHaveLength(EQ_BAND_COUNT);
            for (const g of gains) {
                expect(Math.abs(g), name).toBeLessThanOrEqual(EQ_GAIN_RANGE);
            }
        }
    });

    it('the chip order names only real presets, each with a label key', () => {
        for (const name of PRESET_ORDER) {
            expect(EQ_PRESETS[name]).toBeDefined();
            expect(PRESET_LABEL_KEYS[name]).toBeDefined();
        }
        expect(PRESET_LABEL_KEYS.custom).toBeDefined();
    });
});
