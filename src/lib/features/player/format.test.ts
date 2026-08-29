import { describe, expect, it } from 'vitest';
import { fmtTime, qualityLabel, artInitials } from './format';
import type { Track } from '$lib/models/Track';

function track(overrides: Partial<Track>): Track {
    return {
        id: 1,
        folderId: null,
        path: '/music/a.flac',
        title: 'A',
        artist: null,
        album: null,
        composer: null,
        durationSecs: 0,
        format: 'FLAC',
        sampleRate: null,
        bitDepth: null,
        channels: null,
        lossless: true,
        addedAt: 0,
        ...overrides,
    };
}

describe('fmtTime', () => {
    it('formats minutes and zero-padded seconds', () => {
        expect(fmtTime(0)).toBe('0:00');
        expect(fmtTime(344)).toBe('5:44');
        expect(fmtTime(611)).toBe('10:11');
    });

    it('clamps negatives and tolerates NaN', () => {
        expect(fmtTime(-3)).toBe('0:00');
        expect(fmtTime(Number.NaN)).toBe('0:00');
    });
});

describe('qualityLabel', () => {
    it('marks hi-res when depth exceeds 16 bits', () => {
        expect(qualityLabel(track({ bitDepth: 24, sampleRate: 96000 }))).toBe(
            'Hi-Res · FLAC 24/96',
        );
    });

    it('keeps CD-quality plain with fractional kHz', () => {
        expect(qualityLabel(track({ bitDepth: 16, sampleRate: 44100 }))).toBe('FLAC 16/44.1');
    });

    it('falls back to the bare format without properties', () => {
        expect(qualityLabel(track({ format: 'MP3', lossless: false }))).toBe('MP3');
    });
});

describe('artInitials', () => {
    it('uppercases the first two characters', () => {
        expect(artInitials('Nocturne')).toBe('NO');
        expect(artInitials('')).toBe('?');
    });
});
