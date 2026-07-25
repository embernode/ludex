// "Seen since you last looked" bookkeeping for the detections ledger.
//
// `applications.first_seen_at` has been in the schema since the
// initial migration, so the NEW badge needs no database change — only
// a record of when the user last opened the view. That is a purely
// local, presentational concern, so it lives in localStorage rather
// than on the wire.

const WATERMARK_KEY = 'ludex-detections-seen-at';

/** Milliseconds since the epoch, or `null` if `iso` isn't a date. */
function instant(iso: string | null): number | null {
    if (!iso) return null;
    const ms = Date.parse(iso);
    return Number.isNaN(ms) ? null : ms;
}

/**
 * Whether an application first seen at `firstSeenAt` counts as new
 * relative to `watermark`.
 *
 * With no watermark nothing is new: the first visit has no baseline,
 * and badging every row on the one visit where the list is longest
 * would make the badge meaningless. Both sides are compared as
 * instants — the daemon sends UTC, but a locally stamped watermark
 * may carry an offset, so a lexical comparison would be wrong.
 */
export function isNewSince(
    firstSeenAt: string,
    watermark: string | null,
): boolean {
    const seen = instant(firstSeenAt);
    const since = instant(watermark);
    if (seen === null || since === null) return false;
    return seen > since;
}

/** The stored watermark, or `null` if the view has never been opened. */
export function storedWatermark(): string | null {
    if (typeof localStorage === 'undefined') return null;
    try {
        return localStorage.getItem(WATERMARK_KEY);
    } catch (_) {
        // localStorage blocked — no baseline, so nothing badges.
        return null;
    }
}

/** Record that the user has now seen the ledger as of `iso`. */
export function stampWatermark(iso: string): void {
    try {
        localStorage.setItem(WATERMARK_KEY, iso);
    } catch (_) {
        // Non-fatal: the badge just won't clear across restarts.
    }
}
