// Validation for settings that commit on change rather than behind a
// Save button.
//
// The Save button used to double as the validator: a value the user
// hadn't committed stayed visibly dirty, so a typo was self-evident
// and simply never reached the daemon. Committing on change removes
// that, which means an out-of-range or nonsense entry has to be
// rejected loudly instead — silently clamping it would rewrite the
// user's number to a different one with nothing left on screen to
// show it happened.

export interface Bounds {
    /** Inclusive lower bound. */
    readonly min: number;
    /** Inclusive upper bound. */
    readonly max: number;
}

export type CommitOutcome =
    /** Parsed, in range, and different from what is stored. */
    | { readonly status: 'ok'; readonly value: number }
    /** Parsed and in range, but identical to the stored value. */
    | { readonly status: 'unchanged' }
    /** Rejected — `message` is user-facing. */
    | { readonly status: 'invalid'; readonly message: string };

/**
 * Validate a raw field entry against `bounds` and compare it to
 * `stored`.
 *
 * Fractional entries are floored rather than rejected: every setting
 * this backs is a whole number of seconds, minutes, hours, MiB, or
 * snapshots, and typing `50.9` reads as intent to enter 50 rather
 * than as a mistake. Anything unparseable or outside the bounds is
 * rejected so the caller can revert the field and surface `message`.
 */
export function parseSetting(
    raw: string,
    bounds: Bounds,
    stored: number,
): CommitOutcome {
    const trimmed = raw.trim();

    // Matched before `Number()` rather than relying on it: `Number()`
    // speaks the entire JavaScript numeric-literal grammar, so it
    // happily turns `0x10` into 16, `1e3` into 1000, and `.5` into
    // 0.5. Coercing those would rewrite the user's entry into a
    // different number with nothing on screen to show it, which is
    // precisely what this module refuses to do. Plain decimal only.
    if (!/^-?\d+(\.\d+)?$/.test(trimmed)) {
        return { status: 'invalid', message: 'Enter a number.' };
    }

    const parsed = Number(trimmed);

    const outOfRange = {
        status: 'invalid',
        message: `Enter a number between ${bounds.min} and ${bounds.max}.`,
    } as const;

    // `Number.isFinite` also rejects `Infinity`, which parses cleanly
    // and would otherwise survive a bare `value > max` comparison
    // only by luck of ordering.
    if (!Number.isFinite(parsed)) return outOfRange;

    const value = Math.floor(parsed);
    if (value < bounds.min || value > bounds.max) return outOfRange;

    return value === stored ? { status: 'unchanged' } : { status: 'ok', value };
}
