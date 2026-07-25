import { describe, expect, it } from 'vitest';
import { interactiveShare } from './format';

// Drives the width of every share bar in the library and on the
// app-detail page, so a value outside [0, 100] renders a bar wider
// than its own track.
describe('interactiveShare', () => {
    it('gives the interactive portion as a percentage', () => {
        expect(interactiveShare(50, 100)).toBe(50);
        expect(interactiveShare(91, 100)).toBe(91);
    });

    it('is zero when nothing was interactive', () => {
        expect(interactiveShare(0, 100)).toBe(0);
    });

    it('is full when everything was interactive', () => {
        expect(interactiveShare(100, 100)).toBe(100);
    });

    // A session that recorded no runtime at all — an immediate crash,
    // or a row still being written — must not divide by zero.
    it('is zero when there is no full runtime to divide by', () => {
        expect(interactiveShare(0, 0)).toBe(0);
        expect(interactiveShare(10, 0)).toBe(0);
        expect(interactiveShare(10, -5)).toBe(0);
    });

    // Interactive should never exceed full, but the two are summed by
    // separate paths in the daemon, so the bar clamps rather than
    // trusting the arithmetic.
    it('clamps rather than overflowing the track', () => {
        expect(interactiveShare(150, 100)).toBe(100);
        expect(interactiveShare(-10, 100)).toBe(0);
    });
});
