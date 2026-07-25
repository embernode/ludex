// Geometry for the activity view's "when you played" grid.
//
// Each day is a lane and each session is a block positioned by the
// wall-clock time it occupied. That is deliberately wall-clock:
// unlike a session's *duration* — which the daemon measures against a
// monotonic clock so a suspend contributes nothing — the question here
// is "what time of day do you play", and the answer is only meaningful
// against the clock on the wall.

/** The fields of a session summary this module needs. */
export interface SessionRange {
    readonly started_at: string;
    /** Empty while the session is still open. */
    readonly ended_at: string;
}

/** One session's footprint within one day's lane. */
export interface Block {
    /** Distance from the left edge of the lane, as a percentage. */
    readonly leftPct: number;
    /** Width of the block, as a percentage of the lane. */
    readonly widthPct: number;
    /** Seconds of this day the session occupied, after clipping. */
    readonly seconds: number;
}

/**
 * Clip `session` to the day spanning `[dayStartMs, dayEndMs)`, or
 * `null` when it doesn't overlap that day at all.
 *
 * The end is passed in rather than derived as `start + 24h` because a
 * local day is not always 24 hours: on the two daylight-saving
 * transitions it is 23 or 25. Assuming 24 would leave an hour of a
 * long day in no lane at all, and would place an hour of the short
 * day in two lanes at once — counting it twice in the totals. The
 * daemon buckets daily playtime with SQLite's `localtime`, which gets
 * this right, so a fixed span here would also make the grid and the
 * bars disagree on exactly those two days.
 *
 * A session that ran past midnight is clipped at the boundary and so
 * appears on both days it touched, each showing only the part actually
 * played then. An open session is drawn up to `nowMs` — it hasn't
 * ended, so drawing it to the end of the day would claim play that
 * hasn't happened.
 */
export function clipToDay(
    session: SessionRange,
    dayStartMs: number,
    dayEndMs: number,
    nowMs: number,
): Block | null {
    const span = dayEndMs - dayStartMs;
    if (span <= 0) return null;

    const started = Date.parse(session.started_at);
    if (Number.isNaN(started)) return null;

    const ended = session.ended_at ? Date.parse(session.ended_at) : nowMs;
    if (Number.isNaN(ended)) return null;

    const from = Math.max(started, dayStartMs);
    const to = Math.min(ended, dayEndMs);
    if (to <= from) return null;

    return {
        leftPct: ((from - dayStartMs) / span) * 100,
        widthPct: ((to - from) / span) * 100,
        seconds: Math.round((to - from) / 1000),
    };
}

/** A day row: its lane blocks and the total time they cover. */
export interface DayLane {
    /** Local midnight the lane starts at, in epoch milliseconds. */
    readonly dayStartMs: number;
    /** The next local midnight — exclusive end of the lane. */
    readonly dayEndMs: number;
    readonly blocks: readonly Block[];
    readonly totalSeconds: number;
}

/**
 * Build `dayCount` consecutive lanes ending with the day containing
 * `nowMs`, oldest first.
 *
 * Boundaries are real local midnights, walked with `setDate`, so
 * consecutive lanes are always contiguous and each one spans exactly
 * its own day however long that day happened to be. This matches how
 * the daemon buckets daily playtime — a session played after local
 * midnight belongs to the day the user thinks they played it.
 */
export function buildLanes(
    sessions: readonly SessionRange[],
    dayCount: number,
    nowMs: number,
): DayLane[] {
    const lanes: DayLane[] = [];
    if (dayCount <= 0) return lanes;

    // Midnight beginning the oldest day in the window.
    const cursor = new Date(nowMs);
    cursor.setHours(0, 0, 0, 0);
    cursor.setDate(cursor.getDate() - (dayCount - 1));

    for (let i = 0; i < dayCount; i++) {
        const dayStartMs = cursor.getTime();
        // Advance first, so the end is the next real local midnight
        // rather than a fixed offset from the start.
        cursor.setDate(cursor.getDate() + 1);
        const dayEndMs = cursor.getTime();

        const blocks: Block[] = [];
        let totalSeconds = 0;
        for (const session of sessions) {
            const block = clipToDay(session, dayStartMs, dayEndMs, nowMs);
            if (!block) continue;
            blocks.push(block);
            totalSeconds += block.seconds;
        }
        lanes.push({ dayStartMs, dayEndMs, blocks, totalSeconds });
    }
    return lanes;
}
