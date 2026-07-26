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
 * Last scheme the appearance portal reported. Cached only so the
 * pre-paint bootstrap in `app.html` can seed `auto` with it: the
 * portal answer itself arrives after mount, which on a desktop the
 * media query gets wrong means every launch would paint the wrong
 * scheme and then snap. A stale value costs at most that same frame,
 * because the live answer overwrites it moments later.
 */
const PORTAL_KEY = 'ludex-portal-scheme';

/**
 * Accent choices offered in Settings. All but `graphite` come from the
 * design's own palette prop, so the list is authored rather than
 * invented; `graphite` was added afterwards as a darker companion to
 * `slate`. Note `slate`, `graphite` and `bone` are deliberately
 * near-neutral.
 */
export const ACCENTS: readonly { hex: string; name: string }[] = [
    { hex: '#4fb96a', name: 'green' },
    { hex: '#5aa9c9', name: 'cyan' },
    { hex: '#8f9aa6', name: 'slate' },
    { hex: '#5f6a76', name: 'graphite' },
    { hex: '#cdc9c2', name: 'bone' },
    { hex: '#c8ae72', name: 'sand' },
    { hex: '#a58fd8', name: 'lavender' },
];

/**
 * Accent applied when nothing is stored. Named rather than taken from
 * `ACCENTS[0]` so the swatch order stays the design's and the default
 * is an independent choice — changing one shouldn't silently change
 * the other.
 */
export const DEFAULT_ACCENT = '#5aa9c9';

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

/**
 * The desktop's preference as last reported by the appearance portal,
 * or `null` when the portal expressed none or wasn't reachable.
 */
let portalPreference: Theme | null = null;

/**
 * Decode a portal answer into a scheme.
 *
 * `no-preference` is the desktop explicitly declining to choose, and
 * `unavailable` means nothing answered; both fall through to the
 * media query rather than being read as a preference for dark.
 */
export function preferenceFromPortal(scheme: string): Theme | null {
    if (scheme === 'dark') return 'dark';
    if (scheme === 'light') return 'light';
    return null;
}

/**
 * Record what the desktop asked for. Returns whether the value
 * changed, so callers can skip a needless re-apply.
 */
export function setPortalPreference(scheme: string): boolean {
    const next = preferenceFromPortal(scheme);
    try {
        if (next) localStorage.setItem(PORTAL_KEY, next);
        else localStorage.removeItem(PORTAL_KEY);
    } catch (_) {
        // Non-fatal: only costs the pre-paint seed on the next launch.
    }
    if (next === portalPreference) return false;
    portalPreference = next;
    return true;
}

/** True when the desktop asks for a dark palette. */
function prefersDark(): boolean {
    if (typeof window === 'undefined' || !window.matchMedia) return false;
    return window.matchMedia('(prefers-color-scheme: dark)').matches;
}

/**
 * The scheme a mode paints as right now.
 *
 * `auto` prefers the portal's answer over `prefers-color-scheme`: the
 * media query reports what the *webview* believes, which on KDE Plasma
 * Wayland frequently disagrees with the actual desktop setting. The
 * query remains the fallback for desktops with no portal.
 */
function resolveScheme(mode: ThemeMode): Theme {
    if (mode !== 'auto') return mode;
    if (portalPreference) return portalPreference;
    return prefersDark() ? 'dark' : 'light';
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
 * This is the fallback for desktops with no appearance portal. Where
 * a portal answers, its preference wins — see `setPortalPreference`.
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

/**
 * Resolve an **opaque** CSS colour expression — including `var()`
 * chains and `color-mix()` — to a concrete `rgb(...)` string.
 *
 * Two steps, both necessary. Painting the expression on a throwaway
 * element and reading back `background-color` resolves the custom
 * properties, but a computed colour serialises *in its mixing space*,
 * so anything built with `color-mix(in oklab, …)` comes back as
 * `oklab(…)`. zrender has no parser for that and ECharts silently
 * substitutes opaque black, with the warning compiled out of release
 * builds. Passing the result through a 1x1 canvas forces the engine
 * to convert to sRGB, which is what charts can actually consume.
 *
 * Translucent expressions are refused rather than flattened: painting
 * one onto an empty canvas and reading the pixel back loses the alpha
 * and quantises the channels, which would return a confidently wrong
 * opaque colour. `rgba()` that needs no conversion is passed through
 * untouched.
 *
 * Returns `fallback` if anything is unavailable or unparseable, so a
 * failure degrades to the palette's built-in literals rather than to
 * black.
 */
export function resolveCssColor(expression: string, fallback: string): string {
    if (typeof document === 'undefined') return fallback;
    const probe = document.createElement('span');
    try {
        probe.style.display = 'none';
        probe.style.backgroundColor = expression;
        // Must be in the tree to inherit custom properties from :root.
        document.documentElement.append(probe);
        const computed = getComputedStyle(probe).backgroundColor;

        // An unparseable value leaves the initial `transparent`.
        if (!computed || computed === 'rgba(0, 0, 0, 0)') return fallback;
        // Already sRGB — `rgba()` included, so alpha survives here.
        if (computed.startsWith('rgb')) return computed;
        // A colour in another space carries alpha after a slash.
        if (computed.includes('/')) return fallback;

        const canvas = document.createElement('canvas');
        canvas.width = 1;
        canvas.height = 1;
        const ctx = canvas.getContext('2d', { willReadFrequently: true });
        if (!ctx) return fallback;

        // Assigning an unparseable value to `fillStyle` is specified to
        // be *ignored*, leaving the previous value in place — so a
        // canvas that couldn't parse `computed` would silently paint
        // the default black and we would return it as if it were the
        // answer. Seed a sentinel and check it actually moved.
        const sentinel = '#010203';
        ctx.fillStyle = sentinel;
        ctx.fillStyle = computed;
        if (ctx.fillStyle === sentinel) return fallback;

        ctx.fillRect(0, 0, 1, 1);
        const [r, g, b] = ctx.getImageData(0, 0, 1, 1).data;
        return `rgb(${r}, ${g}, ${b})`;
    } catch (_) {
        return fallback;
    } finally {
        // Also runs on the error paths above, so a throw between the
        // append and the read can't leave probes accumulating on
        // <html> — this is called several times per chart build.
        probe.remove();
    }
}
