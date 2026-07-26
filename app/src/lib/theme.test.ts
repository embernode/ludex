import { describe, expect, it } from 'vitest';
import {
    ACCENTS,
    DEFAULT_ACCENT,
    nextMode,
    preferenceFromPortal,
    storedMode,
} from './theme';

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
    // The default is chosen independently of the ordering, so the
    // invariant worth holding is that it is one of the swatches — a
    // default outside the list would leave the picker showing nothing
    // selected on a fresh profile.
    it('offers the default as one of its swatches', () => {
        expect(ACCENTS.map((a) => a.hex)).toContain(DEFAULT_ACCENT);
    });

    it('offers the authored swatches in the design order', () => {
        expect(ACCENTS).toHaveLength(7);
        expect(ACCENTS.map((a) => a.name)).toEqual([
            'green',
            'cyan',
            'slate',
            'graphite',
            'bone',
            'sand',
            'lavender',
        ]);
    });

    // Two near-neutral greys sit side by side, so they have to be
    // distinguishable as swatches rather than reading as one colour
    // rendered twice.
    it('keeps graphite clearly darker than slate', () => {
        const lum = (hex: string) => {
            const v = (i: number) => parseInt(hex.slice(1 + i * 2, 3 + i * 2), 16);
            return 0.2126 * v(0) + 0.7152 * v(1) + 0.0722 * v(2);
        };
        const slate = ACCENTS.find((a) => a.name === 'slate');
        const graphite = ACCENTS.find((a) => a.name === 'graphite');
        expect(slate && graphite).toBeTruthy();
        expect(lum(slate!.hex) - lum(graphite!.hex)).toBeGreaterThan(25);
    });

    it('gives every swatch a six-digit hex', () => {
        for (const { hex } of ACCENTS) {
            expect(hex).toMatch(/^#[0-9a-f]{6}$/);
        }
    });
});

// The freedesktop appearance portal is authoritative for what the
// desktop wants, unlike `prefers-color-scheme`, which on KDE Plasma
// Wayland frequently disagrees with the actual setting.
describe('preferenceFromPortal', () => {
    it('maps the portal answers to a scheme', () => {
        expect(preferenceFromPortal('dark')).toBe('dark');
        expect(preferenceFromPortal('light')).toBe('light');
    });

    // "No preference" is the desktop declining to choose, which is a
    // different thing from wanting dark — falling through to the media
    // query is the honest response.
    it('yields no preference for the portal saying so', () => {
        expect(preferenceFromPortal('no-preference')).toBeNull();
    });

    it('yields no preference when no portal answered', () => {
        expect(preferenceFromPortal('unavailable')).toBeNull();
        expect(preferenceFromPortal('')).toBeNull();
        expect(preferenceFromPortal('nonsense')).toBeNull();
    });
});
