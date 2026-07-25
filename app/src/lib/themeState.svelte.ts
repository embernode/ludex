// Reactive appearance state, shared by the header control and the
// Settings › Appearance card.
//
// The cycling button appears in two places and both must agree, so
// the mode can't live in either component. Same for the accent: the
// picker writes it, and anything that wants to show the current
// choice reads from here.

import {
    applyAccent,
    applyMode,
    nextMode,
    setPortalPreference,
    storedAccent,
    storedMode,
    type ThemeMode,
} from './theme';

// Seeded from storage at module load. The app is client-rendered
// (`ssr = false`), so this runs in the browser and the first paint
// already agrees with the bootstrap in `app.html`.
let mode = $state<ThemeMode>(storedMode());
let accent = $state<string>(storedAccent());
let portalAnswered = $state<boolean>(false);

export const MODE_LABEL: Record<ThemeMode, string> = {
    dark: 'Dark',
    light: 'Light',
    auto: 'Auto',
};

/**
 * Help text under the scheme control. `auto` is absent on purpose —
 * its note names the scheme it currently resolves to, so the caller
 * composes it rather than reading a fixed string.
 */
export const MODE_NOTE: Record<'dark' | 'light', string> = {
    dark: 'Always dark, regardless of the desktop theme.',
    light: 'Always light, regardless of the desktop theme.',
};

export function currentMode(): ThemeMode {
    return mode;
}

export function currentAccent(): string {
    return accent;
}

/** Advance the scheme one step: dark → light → auto → dark. */
export function cycleMode(): void {
    mode = nextMode(mode);
    applyMode(mode);
}

/** Re-resolve `auto` after the desktop's preference changed. */
export function refreshAuto(): void {
    if (mode === 'auto') applyMode(mode);
}

/**
 * Adopt a colour-scheme answer from the appearance portal, repainting
 * if the app is following the desktop.
 */
export function applyPortalScheme(scheme: string): void {
    if (setPortalPreference(scheme)) refreshAuto();
}

/** Whether the portal, rather than the media query, is driving Auto. */
export function portalIsDriving(): boolean {
    return portalAnswered;
}

/** Record that a portal answer arrived, for the Settings help line. */
export function notePortalAnswered(answered: boolean): void {
    portalAnswered = answered;
}

export function setAccent(hex: string): void {
    accent = hex;
    applyAccent(hex);
}
