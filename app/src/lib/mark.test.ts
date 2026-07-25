import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

// The header draws the brand mark inline rather than as an <img>, so
// its ring can inherit `currentColor` and theme with the chrome. That
// means the geometry exists in two places, and nothing at runtime
// would notice them drifting apart — a redraw of the master would
// silently leave the header on the old mark.
const MASTER = readFileSync('../assets/logo.svg', 'utf8');
const LAYOUT = readFileSync('src/routes/+layout.svelte', 'utf8');

/** Collapse whitespace so formatting differences don't count as drift. */
const flat = (s: string) => s.replace(/\s+/g, ' ');

describe('the header mark matches the master', () => {
    // Anchored on a preceding space, and the path on its leading
    // `M`: an unanchored /d="([^"]+)"/ happily matches the `d="true"`
    // inside `aria-hidden="true"` and compares the wrong thing.
    it.each([
        ['ring centre x', /\scx="([^"]+)"/],
        ['ring centre y', /\scy="([^"]+)"/],
        ['ring radius', /\sr="([^"]+)"/],
        ['stroke width', /\sstroke-width="([^"]+)"/],
        ['dash pattern', /\sstroke-dasharray="([^"]+)"/],
        ['dash offset', /\sstroke-dashoffset="([^"]+)"/],
        ['triangle path', /\sd="(M[^"]+)"/],
    ])('agrees on the %s', (_label, pattern) => {
        const master = MASTER.match(pattern)?.[1];
        const header = LAYOUT.match(pattern)?.[1];
        expect(master).toBeDefined();
        expect(header).toBe(master);
    });

    it('keeps the ring themeable and the triangle fixed', () => {
        expect(flat(MASTER)).toContain('stroke="currentColor"');
        expect(flat(LAYOUT)).toContain('stroke="currentColor"');
        // The master carries the literal; the header reads the token
        // that holds it, so that the one colour which must not follow
        // the accent picker is declared in exactly one place.
        expect(flat(MASTER)).toContain('fill="#6ec46e"');
        expect(flat(LAYOUT)).toContain('fill="var(--brand-green)"');
    });
});
