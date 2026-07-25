import { describe, expect, it } from 'vitest';
import { ACCENTS, DEFAULT_ACCENT, nextMode, storedMode } from './theme';

describe('nextMode', () => {
    // The single cycling control replaces a tri-state segment, so the
    // order is the design's left-to-right reading order rather than
    // an arbitrary rotation. Getting it wrong is invisible in review
    // but immediately wrong in the hand.
    it('cycles dark to light to auto and back', () => {
        expect(nextMode('dark')).toBe('light');
        expect(nextMode('light')).toBe('auto');
        expect(nextMode('auto')).toBe('dark');
    });

    it('returns to the starting mode after three steps', () => {
        expect(nextMode(nextMode(nextMode('dark')))).toBe('dark');
    });
});

describe('storedMode', () => {
    // Node's WebStorage is enabled under vitest, so this exercises the
    // empty-store path (`getItem` returns null), not the
    // storage-unavailable guard. Both land on the same default, which
    // is the behaviour that matters: a profile with nothing saved
    // follows the desktop rather than forcing a scheme.
    it('defaults to auto when nothing is stored', () => {
        expect(storedMode()).toBe('auto');
    });
});

describe('ACCENTS', () => {
    it('offers the six authored swatches, green first', () => {
        expect(ACCENTS).toHaveLength(6);
        expect(ACCENTS[0].hex).toBe(DEFAULT_ACCENT);
        expect(ACCENTS.map((a) => a.name)).toEqual([
            'green',
            'cyan',
            'slate',
            'bone',
            'sand',
            'lavender',
        ]);
    });

    it('gives every swatch a six-digit hex', () => {
        for (const { hex } of ACCENTS) {
            expect(hex).toMatch(/^#[0-9a-f]{6}$/);
        }
    });
});
