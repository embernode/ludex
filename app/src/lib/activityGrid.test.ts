import { describe, expect, it } from 'vitest';
import { buildLanes, clipToDay } from './activityGrid';

const DAY = 24 * 3600 * 1000;
// An arbitrary local midnight; the maths is relative to it, so the
// actual zone doesn't matter.
const dayStart = new Date('2026-07-20T00:00:00Z').getTime();
const at = (hours: number) => new Date(dayStart + hours * 3600_000).toISOString();
const dayEnd = dayStart + DAY;
const now = dayStart + 23 * 3600_000;

describe('clipToDay', () => {
    it('places a session inside the day by its start and length', () => {
        const block = clipToDay(
            { started_at: at(20), ended_at: at(23) },
            dayStart,
            dayEnd,
            now,
        );
        expect(block).not.toBeNull();
        expect(block?.leftPct).toBeCloseTo((20 / 24) * 100, 5);
        expect(block?.widthPct).toBeCloseTo((3 / 24) * 100, 5);
        expect(block?.seconds).toBe(3 * 3600);
    });

    it('ignores a session that ended before the day began', () => {
        expect(
            clipToDay({ started_at: at(-5), ended_at: at(-2) }, dayStart, dayEnd, now),
        ).toBeNull();
    });

    it('ignores a session that starts after the day ended', () => {
        expect(
            clipToDay({ started_at: at(25), ended_at: at(26) }, dayStart, dayEnd, now),
        ).toBeNull();
    });

    // A play that ran past midnight occupies both days it touched, and
    // the portion drawn on each must be the portion actually played
    // then — otherwise an evening session inflates the next morning.
    it('clips a session that began before the day to the day boundary', () => {
        const block = clipToDay(
            { started_at: at(-2), ended_at: at(1) },
            dayStart,
            dayEnd,
            now,
        );
        expect(block?.leftPct).toBe(0);
        expect(block?.widthPct).toBeCloseTo((1 / 24) * 100, 5);
        expect(block?.seconds).toBe(3600);
    });

    it('clips a session that runs past midnight to the end of the day', () => {
        const block = clipToDay(
            { started_at: at(23), ended_at: at(26) },
            dayStart,
            dayEnd,
            now,
        );
        expect(block?.leftPct).toBeCloseTo((23 / 24) * 100, 5);
        expect(block?.widthPct).toBeCloseTo((1 / 24) * 100, 5);
    });

    // An open session has no end; it is still running, so it is drawn
    // up to the present rather than to the end of the day.
    it('draws an open session up to now', () => {
        const block = clipToDay(
            { started_at: at(21), ended_at: '' },
            dayStart,
            dayEnd,
            now,
        );
        expect(block?.widthPct).toBeCloseTo((2 / 24) * 100, 5);
    });

    it('never lets a block escape its lane', () => {
        const block = clipToDay(
            { started_at: at(-10), ended_at: at(40) },
            dayStart,
            dayEnd,
            dayEnd,
        );
        expect(block?.leftPct).toBe(0);
        expect(block?.widthPct).toBe(100);
    });

    it('ignores an unparseable timestamp rather than drawing at zero', () => {
        expect(
            clipToDay({ started_at: 'nonsense', ended_at: at(2) }, dayStart, dayEnd, now),
        ).toBeNull();
    });

    // A local day is 23 or 25 hours on the daylight-saving
    // transitions. Positions are relative to the lane's own span, so
    // a short day must still place noon near the middle rather than
    // leaking past the end.
    it('scales to a short daylight-saving day', () => {
        const shortEnd = dayStart + 23 * 3600_000;
        const block = clipToDay(
            { started_at: at(22), ended_at: at(23) },
            dayStart,
            shortEnd,
            shortEnd,
        );
        expect(block?.leftPct).toBeCloseTo((22 / 23) * 100, 5);
        expect(block?.widthPct).toBeCloseTo((1 / 23) * 100, 5);
    });

    it('scales to a long daylight-saving day', () => {
        const longEnd = dayStart + 25 * 3600_000;
        const block = clipToDay(
            { started_at: at(24), ended_at: at(25) },
            dayStart,
            longEnd,
            longEnd,
        );
        expect(block).not.toBeNull();
        expect(block?.leftPct).toBeCloseTo((24 / 25) * 100, 5);
    });
});

describe('buildLanes', () => {
    const now = new Date('2026-07-25T18:00:00Z').getTime();

    it('returns one lane per requested day, oldest first', () => {
        const lanes = buildLanes([], 7, now);
        expect(lanes).toHaveLength(7);
        for (let i = 1; i < lanes.length; i++) {
            expect(lanes[i].dayStartMs).toBeGreaterThan(lanes[i - 1].dayStartMs);
        }
    });

    // Contiguity is the property that survives a DST transition: a gap
    // would drop play into no lane, an overlap would count it twice.
    it('makes consecutive lanes exactly contiguous', () => {
        const lanes = buildLanes([], 400, now);
        for (let i = 1; i < lanes.length; i++) {
            expect(lanes[i].dayStartMs).toBe(lanes[i - 1].dayEndMs);
        }
    });

    it('gives every lane a positive span', () => {
        for (const lane of buildLanes([], 400, now)) {
            expect(lane.dayEndMs).toBeGreaterThan(lane.dayStartMs);
        }
    });

    it('sums only the part of a session that fell inside each lane', () => {
        const lanes = buildLanes(
            [
                {
                    started_at: new Date(now - 3600_000).toISOString(),
                    ended_at: new Date(now).toISOString(),
                },
            ],
            2,
            now,
        );
        expect(lanes[0].totalSeconds).toBe(0);
        expect(lanes[1].totalSeconds).toBe(3600);
    });

    it('returns nothing for a non-positive day count', () => {
        expect(buildLanes([], 0, now)).toEqual([]);
    });
});
