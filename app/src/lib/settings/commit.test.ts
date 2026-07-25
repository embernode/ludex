import { describe, expect, it } from 'vitest';
import { parseSetting } from './commit';

// The settings UI commits on change rather than behind a Save button.
// That removes the old safety net: previously a nonsense value simply
// never saved, because the user could see the field was still dirty.
// With instant apply, anything not caught here is either silently
// discarded or silently sent to the daemon — so rejection has to be
// explicit and reportable.
describe('parseSetting', () => {
    const bounds = { min: 1, max: 16384 };

    it('accepts an in-range integer', () => {
        expect(parseSetting('50', bounds, 10)).toEqual({
            status: 'ok',
            value: 50,
        });
    });

    it('reports no change when the value matches what is stored', () => {
        expect(parseSetting('50', bounds, 50)).toEqual({ status: 'unchanged' });
    });

    it('ignores surrounding whitespace', () => {
        expect(parseSetting('  50  ', bounds, 10)).toEqual({
            status: 'ok',
            value: 50,
        });
    });

    it('floors a fractional entry rather than rejecting it', () => {
        expect(parseSetting('50.9', bounds, 10)).toEqual({
            status: 'ok',
            value: 50,
        });
    });

    it('rejects an empty field', () => {
        expect(parseSetting('', bounds, 10)).toEqual({
            status: 'invalid',
            message: 'Enter a number.',
        });
    });

    it('rejects a non-numeric entry', () => {
        expect(parseSetting('abc', bounds, 10)).toEqual({
            status: 'invalid',
            message: 'Enter a number.',
        });
    });

    // Rejected rather than clamped: silently rewriting the user's
    // input to a different number is the failure mode instant apply
    // makes invisible, since there is no dirty state left to notice.
    it('rejects a value below the minimum, naming the bound', () => {
        expect(parseSetting('0', bounds, 10)).toEqual({
            status: 'invalid',
            message: 'Enter a number between 1 and 16384.',
        });
    });

    it('rejects a value above the maximum, naming the bound', () => {
        expect(parseSetting('99999', bounds, 10)).toEqual({
            status: 'invalid',
            message: 'Enter a number between 1 and 16384.',
        });
    });

    it('rejects a negative entry when the minimum is zero', () => {
        expect(parseSetting('-1', { min: 0, max: 600 }, 30)).toEqual({
            status: 'invalid',
            message: 'Enter a number between 0 and 600.',
        });
    });

    it('accepts zero when the minimum is zero', () => {
        expect(parseSetting('0', { min: 0, max: 600 }, 30)).toEqual({
            status: 'ok',
            value: 0,
        });
    });

    it('rejects the word Infinity', () => {
        expect(parseSetting('Infinity', bounds, 10)).toEqual({
            status: 'invalid',
            message: 'Enter a number.',
        });
    });

    // A long enough run of digits is shaped like a valid entry but
    // overflows to Infinity, which would sail past a bounds test
    // written as `value > max`.
    it('rejects a digit string that overflows to Infinity', () => {
        expect(parseSetting('9'.repeat(400), bounds, 10)).toEqual({
            status: 'invalid',
            message: 'Enter a number between 1 and 16384.',
        });
    });

    // `Number()` speaks the whole JavaScript numeric-literal grammar,
    // so these all coerce to something plausible. Accepting them would
    // silently rewrite what the user typed into a different number —
    // the exact failure this module exists to prevent.
    it.each(['0x10', '0b1010', '0o17', '1e3', '+5', '.5'])(
        'rejects the numeric-literal form %s',
        (raw) => {
            expect(parseSetting(raw, bounds, 10)).toEqual({
                status: 'invalid',
                message: 'Enter a number.',
            });
        },
    );

    it.each(['1,5', '5px', '1_0', '٥', '１２'])(
        'rejects the non-decimal entry %s',
        (raw) => {
            expect(parseSetting(raw, bounds, 10)).toEqual({
                status: 'invalid',
                message: 'Enter a number.',
            });
        },
    );
});
