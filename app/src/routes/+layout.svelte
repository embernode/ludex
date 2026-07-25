<script lang="ts">
    import { onMount } from 'svelte';
    import { page } from '$app/state';
    import ThemeCycleButton from '$lib/ThemeCycleButton.svelte';
    import { watchSystemScheme } from '$lib/theme';
    import { refreshAuto } from '$lib/themeState.svelte';

    let { children } = $props();

    // While on `auto`, follow the desktop for as long as the window is
    // open — not just at startup.
    onMount(() => watchSystemScheme(refreshAuto));

    // Mark a nav link active when the current route matches.
    // `startsWith` rather than `===` so nested routes (e.g.
    // `/app/42`) keep the `Apps` link active.
    function isActive(path: string): boolean {
        if (path === '/')
            return (
                page.url.pathname === '/' || page.url.pathname.startsWith('/app/')
            );
        return (
            page.url.pathname === path || page.url.pathname.startsWith(`${path}/`)
        );
    }
</script>

<div class="app">
    <nav>
        <a class="brand" href="/">ludex</a>
        <div class="links">
            <a href="/" class:active={isActive('/')}>Games</a>
            <a href="/dashboard" class:active={isActive('/dashboard')}>Dashboard</a>
            <a href="/recent" class:active={isActive('/recent')}>Recent</a>
            <a href="/settings" class:active={isActive('/settings')}>Settings</a>
        </div>
        <ThemeCycleButton />
    </nav>
    <div class="content">
        {@render children?.()}
    </div>
</div>

<style>
    /* Colour tokens.
       ----------------------------------------------------------------
       The cool-graphite scale below is the design's own token set,
       transcribed value-for-value (names kebab-cased to match the
       rest of this stylesheet). Light is deliberately *not* an
       inversion of dark: dark puts the page below the surfaces
       (#0b0d0f page, #14181b cards), light puts it above (#eef1f3
       page, #ffffff cards).

       `--raw` is the accent, and it is the single input the user can
       change — every accent-derived token is a `color-mix` on it, so
       setting that one property re-tints the whole UI. Keep it that
       way; resolving an accent to a literal anywhere in CSS silently
       decouples that element from the picker.

       The second block maps the older token names onto this scale so
       pages that haven't been rebuilt against the new design still
       paint from one palette. It is declared once, in `:root` only:
       custom properties substitute at use time, so `var(--bg)` inside
       these resolves per scheme without a dark-mode duplicate. */
    :global(:root) {
        /* Tell the browser which palette to render native UA
           controls in (<select> dropdown, scrollbars, date
           pickers). Without this the dropdown list paints from the
           system light theme even when ludex is in dark mode. */
        color-scheme: light;

        --raw: #4fb96a;
        --bg: #eef1f3;
        --chrome: #ffffff;
        --surface: #ffffff;
        --tile: #e7ebee;
        --lane: #e9edef;
        --line: rgba(20, 26, 32, 0.13);
        --hair: rgba(20, 26, 32, 0.08);
        --lane-line: rgba(20, 26, 32, 0.07);
        --track: rgba(20, 26, 32, 0.1);
        --fg: #1b2126;
        --fg2: #5a6469;
        --fg3: #828b91;
        --ac: color-mix(in oklab, var(--raw) 62%, #20262b);
        --ac-idle: color-mix(in oklab, var(--ac) 26%, #ffffff);
        --pill-bd: color-mix(in oklab, var(--ac) 45%, transparent);
        --pill-bg: color-mix(in oklab, var(--ac) 14%, transparent);
        --pill-fg: color-mix(in oklab, var(--ac) 70%, #10161a);

        /* Scheme-independent. `--brand-green` is the daemon-health dot
           in Settings (and, once it lands, the logo triangle): both
           are fixed brand/health signals that must not follow the
           accent picker — picking lavender must not turn "daemon
           running" lavender. `--warn` is the destructive-action tint. */
        --warn: #e08b6a;
        --brand-green: #6ec46e;

        /* --- bridge: older token names onto the scale above --- */
        --bg-page: var(--bg);
        --bg-surface: var(--surface);
        --bg-nav: var(--chrome);
        --bg-hover: var(--tile);
        --border: var(--line);
        --border-strong: var(--fg3);
        --border-soft: var(--hair);
        --text-primary: var(--fg);
        --text-secondary: var(--fg);
        --text-body: var(--fg);
        --text-muted: var(--fg2);
        --text-subtle: var(--fg3);
        --text-label: var(--fg2);
        --accent: var(--ac);
        --status-open: var(--ac);
        --button-bg: var(--tile);
        --button-border: var(--line);
        --button-text: var(--fg);
        /* Nudged toward the foreground rather than set to `--track`:
           that token is translucent, and over an opaque `--tile`
           button it composites to a ~3/255 difference in light mode
           — no visible hover at all. */
        --button-hover-bg: color-mix(in srgb, var(--tile), var(--fg) 8%);
        --active-bg: var(--tile);
        --code-bg: var(--tile);
        --code-text: var(--fg);
        --empty-border: var(--line);

        /* Error states have no equivalent in the design's token set —
           they stay their own semantic red rather than being folded
           into the graphite scale. */
        --error-bg: #fef2f2;
        --error-border: #fecaca;
        --error-text: #991b1b;
        --row-shadow: rgba(0, 0, 0, 0.04);
    }

    :global(:root[data-theme='dark']) {
        color-scheme: dark;

        --bg: #0b0d0f;
        --chrome: #13171a;
        --surface: #14181b;
        --tile: #1c2124;
        --lane: #111517;
        --line: rgba(255, 255, 255, 0.09);
        --hair: rgba(255, 255, 255, 0.055);
        --lane-line: rgba(255, 255, 255, 0.055);
        --track: rgba(255, 255, 255, 0.08);
        --fg: #e7ebee;
        --fg2: #8b9298;
        --fg3: #6e767c;
        --ac: var(--raw);
        --ac-idle: color-mix(in oklab, var(--ac) 34%, #101416);
        --pill-bd: color-mix(in oklab, var(--ac) 45%, transparent);
        --pill-bg: color-mix(in oklab, var(--ac) 12%, transparent);
        --pill-fg: color-mix(in oklab, var(--ac) 52%, #f2f6f4);

        --error-bg: #1a0606;
        --error-border: #7f1d1d;
        --error-text: #fca5a5;
        --row-shadow: rgba(0, 0, 0, 0.4);
    }

    :global(html, body) {
        margin: 0;
        padding: 0;
        height: 100%;
        font-family: system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif;
        background: var(--bg-page);
        color: var(--text-body);
    }

    :global(button) {
        font: inherit;
        padding: 0.4rem 0.9rem;
        border: 1px solid var(--button-border);
        background: var(--button-bg);
        border-radius: 6px;
        cursor: pointer;
        color: var(--button-text);
    }

    :global(button:hover:not(:disabled)) {
        background: var(--button-hover-bg);
    }

    :global(button:disabled) {
        opacity: 0.5;
        cursor: default;
    }

    :global(code) {
        font-family: 'JetBrains Mono', ui-monospace, monospace;
        font-size: 0.9em;
        background: var(--code-bg);
        color: var(--code-text);
        padding: 0.05em 0.35em;
        border-radius: 4px;
    }

    :global(.hint) {
        color: var(--text-muted);
        font-size: 0.9rem;
    }

    :global(.error) {
        background: var(--error-bg);
        border: 1px solid var(--error-border);
        border-radius: 6px;
        padding: 1rem;
    }

    :global(.error p) {
        margin: 0.25rem 0;
    }

    :global(.error .detail) {
        font-family: 'JetBrains Mono', ui-monospace, monospace;
        font-size: 0.85rem;
        color: var(--error-text);
    }

    :global(.empty) {
        border: 1px dashed var(--empty-border);
        border-radius: 6px;
        padding: 1.5rem;
        text-align: center;
    }

    /* Visually-hide a label that's only there for screen readers
       (a sortable column with no visible header text, etc.).
       Lifted from per-page declarations so it lives in one place. */
    :global(.visually-hidden) {
        position: absolute;
        width: 1px;
        height: 1px;
        padding: 0;
        margin: -1px;
        overflow: hidden;
        clip: rect(0, 0, 0, 0);
        white-space: nowrap;
        border: 0;
    }

    /* A button that visually reads as a link — used wherever an
       inline `<a>` would be wrong (we need a button so the action
       can run JavaScript, but we want link-shaped chrome). */
    :global(.link-button) {
        background: none;
        border: none;
        padding: 0;
        color: var(--accent);
        font: inherit;
        cursor: pointer;
        text-align: left;
    }

    :global(.link-button:hover) {
        text-decoration: underline;
    }

    /* Card chrome for Settings panels that haven't been rebuilt
       against the redesign yet — currently just the blocked-games
       card, which the Detections view replaces. Rebuilt cards use
       the `SettingsCard` / `SettingRow` components instead, which
       own their own styles. Retire this block with the last
       consumer. */
    :global(.settings-card) {
        background: var(--bg-surface);
        border: 1px solid var(--border);
        border-radius: 8px;
        padding: 1.25rem 1.5rem;
        margin-bottom: 1rem;
    }

    :global(.settings-card h2) {
        font-size: 1rem;
        font-weight: 600;
        color: var(--text-label);
        margin: 0 0 0.5rem;
    }

    :global(.settings-card .description) {
        color: var(--text-muted);
        font-size: 0.88rem;
        margin: 0 0 1rem;
        line-height: 1.5;
    }

    :global(.settings-card input[type='search']),
    :global(.settings-card select) {
        font: inherit;
        padding: 0.45rem 0.6rem;
        border: 1px solid var(--button-border);
        background: var(--bg-surface);
        color: var(--text-primary);
        border-radius: 6px;
        font-variant-numeric: tabular-nums;
    }

    :global(.settings-card input[type='search']:focus),
    :global(.settings-card select:focus) {
        outline: 2px solid var(--accent);
        outline-offset: -1px;
    }

    /* Inline variant of the global `.error` block used for
       per-action failures so the form stays visible. Rendered
       above the cards in the Settings page wrapper. */
    :global(.error.inline) {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 1rem;
        padding: 0.6rem 0.9rem;
        margin-bottom: 1rem;
    }

    .app {
        min-height: 100vh;
        display: flex;
        flex-direction: column;
    }

    nav {
        display: flex;
        align-items: center;
        gap: 1.5rem;
        padding: 0.9rem 2rem;
        border-bottom: 1px solid var(--border);
        background: var(--bg-nav);
    }

    .brand {
        font-size: 1.15rem;
        font-weight: 600;
        color: var(--text-primary);
        letter-spacing: -0.01em;
        text-decoration: none;
    }

    .links {
        display: flex;
        gap: 0.25rem;
        flex: 1;
    }

    .links a {
        padding: 0.35rem 0.8rem;
        border-radius: 6px;
        color: var(--text-muted);
        text-decoration: none;
        font-size: 0.95rem;
        transition:
            background 120ms,
            color 120ms;
    }

    .links a:hover {
        background: var(--border-soft);
        color: var(--text-primary);
    }

    .links a.active {
        background: var(--active-bg);
        color: var(--text-primary);
        font-weight: 500;
    }

    .content {
        flex: 1;
    }
</style>
