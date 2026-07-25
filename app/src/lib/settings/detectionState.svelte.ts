// Shared state for the focus-pause setting.
//
// The switch that changes it lives in the Detection card, but the
// Grace-windows card needs it too: alt-tab grace is meaningless when
// focus loss never pauses, so that field disables itself when the
// setting is off. Two private copies would drift the moment the user
// flipped the switch — the Grace card would keep whatever it read at
// mount until the page was re-entered.

import { getPauseWhenBackgrounded, setPauseWhenBackgrounded } from '$lib/api';

let pauseWhenBackgrounded = $state<boolean>(true);

export function pausesOnFocusLoss(): boolean {
    return pauseWhenBackgrounded;
}

/** Re-read from the daemon. Called on mount and on reconnect. */
export async function loadPauseWhenBackgrounded(): Promise<void> {
    pauseWhenBackgrounded = await getPauseWhenBackgrounded();
}

/**
 * Persist `next`, flipping optimistically so the switch animates.
 * Reverts and rethrows if the daemon refused — leaving it flipped
 * would claim a setting that isn't actually in effect.
 */
export async function setPausesOnFocusLoss(next: boolean): Promise<void> {
    const previous = pauseWhenBackgrounded;
    pauseWhenBackgrounded = next;
    try {
        await setPauseWhenBackgrounded(next);
    } catch (e) {
        pauseWhenBackgrounded = previous;
        throw e;
    }
}
