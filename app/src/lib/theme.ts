// Appearance state: a colour-scheme *mode* and an accent colour.
//
// The mode is one of `dark` / `light` / `auto`; `auto` follows the
// desktop. What the rest of the app cares about is the *resolved*
// scheme, which lives as a `data-theme` attribute on <html>, stamped
// by the inline bootstrap in app.html and by the cycling control in
// the layout. Anything that needs to react to it — for example,
// redrawing ECharts with theme-aware colors — observes that attribute
// through this module without importing anything layout-specific.
//
// The accent is a single hex written to the `--raw` custom property.
// Every accent-derived token in the stylesheet is a `color-mix` on
// `--raw`, so setting that one property re-tints the whole UI.

/** Resolved colour scheme — what is actually painted. */
export type Theme = 'light' | 'dark';

/** User-selected mode. `auto` resolves against the desktop. */
export type ThemeMode = 'dark' | 'light' | 'auto';

const MODE_KEY = 'ludex-theme';
const ACCENT_KEY = 'ludex-accent';

/**
 * Accent choices offered in Settings. The hexes come from the design's
 * own palette prop, so this list is authored rather than invented.
 * Note `slate` and `bone` are deliberately near-neutral.
 */
export const ACCENTS: readonly { hex: string; name: string }[] = [
    { hex: '#4fb96a', name: 'green' },
    { hex: '#5aa9c9', name: 'cyan' },
    { hex: '#8f9aa6', name: 'slate' },
    { hex: '#cdc9c2', name: 'bone' },
    { hex: '#c8ae72', name: 'sand' },
    { hex: '#a58fd8', name: 'lavender' },
];

export const DEFAULT_ACCENT = ACCENTS[0].hex;

/** Order the cycling control steps through, following the design. */
const NEXT_MODE: Record<ThemeMode, ThemeMode> = {
    dark: 'light',
    light: 'auto',
    auto: 'dark',
};

function isMode(value: unknown): value is ThemeMode {
    return value === 'dark' || value === 'light' || value === 'auto';
}

/** The mode after one click of the cycling control. */
export function nextMode(mode: ThemeMode): ThemeMode {
    return NEXT_MODE[mode];
}

/**
 * Mode as persisted by a previous session. Defaults to `auto`, which
 * is what the app already did implicitly on a fresh profile — it fell
 * back to `prefers-color-scheme` when nothing was saved. The
 * difference now is that `auto` keeps following the desktop instead
 * of only seeding the first launch.
 */
export function storedMode(): ThemeMode {
    if (typeof localStorage === 'undefined') return 'auto';
    try {
        const saved = localStorage.getItem(MODE_KEY);
        return isMode(saved) ? saved : 'auto';
    } catch (_) {
        // localStorage blocked; fall back to following the desktop.
        return 'auto';
    }
}

/** True when the desktop asks for a dark palette. */
function prefersDark(): boolean {
    if (typeof window === 'undefined' || !window.matchMedia) return false;
    return window.matchMedia('(prefers-color-scheme: dark)').matches;
}

/** The scheme a mode paints as right now. */
function resolveScheme(mode: ThemeMode): Theme {
    if (mode === 'auto') return prefersDark() ? 'dark' : 'light';
    return mode;
}

/**
 * Persist `mode`, stamp the resolved scheme on <html>, and return it.
 * Persistence is best-effort: with localStorage blocked the choice
 * still applies for this session, it just won't survive a restart.
 */
export function applyMode(mode: ThemeMode): Theme {
    const scheme = resolveScheme(mode);
    if (typeof document !== 'undefined') {
        document.documentElement.dataset.theme = scheme;
    }
    try {
        localStorage.setItem(MODE_KEY, mode);
    } catch (_) {
        // Non-fatal — see above.
    }
    return scheme;
}

/**
 * Call `callback` whenever the desktop's colour-scheme preference
 * flips. Only meaningful while the mode is `auto`; callers are
 * expected to check. Returns a disposer.
 *
 * This is the `prefers-color-scheme` approximation of Auto. The
 * design asks for the freedesktop appearance portal, which is a
 * separate piece of work in the Tauri host — see the redesign plan.
 */
export function watchSystemScheme(callback: (theme: Theme) => void): () => void {
    if (typeof window === 'undefined' || !window.matchMedia) return () => {};
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    const onChange = () => callback(mq.matches ? 'dark' : 'light');
    mq.addEventListener('change', onChange);
    return () => mq.removeEventListener('change', onChange);
}

function isHex(value: unknown): value is string {
    return typeof value === 'string' && /^#[0-9a-fA-F]{6}$/.test(value);
}

/** Accent as persisted by a previous session. */
export function storedAccent(): string {
    if (typeof localStorage === 'undefined') return DEFAULT_ACCENT;
    try {
        const saved = localStorage.getItem(ACCENT_KEY);
        return isHex(saved) ? saved : DEFAULT_ACCENT;
    } catch (_) {
        return DEFAULT_ACCENT;
    }
}

/** Persist `hex` and re-tint the UI by setting `--raw` on <html>. */
export function applyAccent(hex: string): void {
    if (typeof document !== 'undefined') {
        document.documentElement.style.setProperty('--raw', hex);
    }
    try {
        localStorage.setItem(ACCENT_KEY, hex);
    } catch (_) {
        // Non-fatal — see applyMode.
    }
}

/** Current theme resolved from `<html data-theme>`. */
export function currentTheme(): Theme {
    if (typeof document === 'undefined') return 'light';
    return document.documentElement.dataset.theme === 'dark' ? 'dark' : 'light';
}

/**
 * Invoke `callback` with the current theme immediately, then again
 * every time `<html data-theme>` changes. Returns a disposer that
 * disconnects the observer — call it from the component's
 * `onMount` cleanup.
 */
export function observeTheme(callback: (theme: Theme) => void): () => void {
    if (typeof document === 'undefined') return () => {};
    callback(currentTheme());
    const obs = new MutationObserver(() => callback(currentTheme()));
    obs.observe(document.documentElement, {
        attributes: true,
        attributeFilter: ['data-theme'],
    });
    return () => obs.disconnect();
}
