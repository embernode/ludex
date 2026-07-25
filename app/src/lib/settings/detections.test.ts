import { describe, expect, it } from 'vitest';
import { isNewSince } from './detections';

// The NEW badge marks applications the gate has accepted since the
// last time the user opened this view. `first_seen_at` has always been
// in the schema; the watermark is the only new concept, and it is a
// localStorage key rather than a column.
describe('isNewSince', () => {
    const watermark = '2026-07-20T12:00:00Z';

    it('marks an application first seen after the watermark', () => {
        expect(isNewSince('2026-07-21T09:00:00Z', watermark)).toBe(true);
    });

    it('does not mark one first seen before the watermark', () => {
        expect(isNewSince('2026-07-19T09:00:00Z', watermark)).toBe(false);
    });

    it('does not mark one first seen exactly at the watermark', () => {
        expect(isNewSince(watermark, watermark)).toBe(false);
    });

    // The first ever visit has no baseline to compare against. Marking
    // every application NEW would make the badge meaningless on the
    // one visit where the list is longest.
    it('marks nothing new when there is no watermark yet', () => {
        expect(isNewSince('2026-07-21T09:00:00Z', null)).toBe(false);
    });

    it('treats an unparseable timestamp as not new', () => {
        expect(isNewSince('not a date', watermark)).toBe(false);
        expect(isNewSince('', watermark)).toBe(false);
    });

    it('treats an unparseable watermark as no baseline', () => {
        expect(isNewSince('2026-07-21T09:00:00Z', 'garbage')).toBe(false);
    });

    // The daemon sends UTC; a watermark stamped from the browser is
    // also an ISO string, but the two must compare as instants rather
    // than lexically.
    it('compares instants, not strings', () => {
        expect(isNewSince('2026-07-20T13:00:00+02:00', watermark)).toBe(false);
        expect(isNewSince('2026-07-20T13:00:00Z', watermark)).toBe(true);
    });
});
