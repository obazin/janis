import type { Track } from '$lib/models/Track';

/** Seconds → "m:ss" (floors, clamps negatives to 0). */
export function fmtTime(seconds: number): string {
    const s = Math.max(0, Math.floor(seconds || 0));
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}

/**
 * The quality badge line for a track, e.g. "Hi-Res · FLAC 24/96",
 * "FLAC 16/44.1", "MP3". Hi-Res = more than 16-bit or above 48 kHz.
 */
export function qualityLabel(track: Track): string {
    const parts: string[] = [];
    const hiRes =
        (track.bitDepth != null && track.bitDepth > 16) ||
        (track.sampleRate != null && track.sampleRate > 48000);
    if (hiRes) parts.push('Hi-Res');
    let format = track.format;
    if (track.bitDepth != null && track.sampleRate != null) {
        const khz = track.sampleRate / 1000;
        const rate = Number.isInteger(khz) ? String(khz) : khz.toFixed(1);
        format += ` ${track.bitDepth}/${rate}`;
    }
    parts.push(format);
    return parts.join(' · ');
}

/** Monogram for generated art: first two characters of the title. */
export function artInitials(title: string): string {
    return (title || '?').slice(0, 2).toUpperCase();
}
