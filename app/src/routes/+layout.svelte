<script lang="ts">
    import { onMount } from 'svelte';
    import { page } from '$app/state';

    let { children } = $props();

    // Mirrors the `data-theme` attribute on <html>. Initialised from
    // the DOM because the inline script in `app.html` has already
    // resolved the effective theme before Svelte hydrates — reading
    // from there keeps us in sync without re-running the media
    // query or localStorage lookup.
    let theme = $state<'light' | 'dark'>('light');

    onMount(() => {
        const current = document.documentElement.dataset.theme;
        theme = current === 'dark' ? 'dark' : 'light';
    });

    function toggleTheme() {
        theme = theme === 'dark' ? 'light' : 'dark';
        document.documentElement.dataset.theme = theme;
        try {
            localStorage.setItem('ludex-theme', theme);
        } catch (_) {
            // User disabled localStorage — the toggle still works for
            // the current session, just won't persist across restarts.
        }
    }

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
        <button
            class="theme-toggle"
            type="button"
            onclick={toggleTheme}
            aria-label={theme === 'dark' ? 'Switch to light mode' : 'Switch to dark mode'}
            title={theme === 'dark' ? 'Switch to light mode' : 'Switch to dark mode'}
        >
            {#if theme === 'dark'}
                <!-- Sun: currently dark, click for light. -->
                <svg
                    xmlns="http://www.w3.org/2000/svg"
                    width="16"
                    height="16"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    aria-hidden="true"
                >
                    <circle cx="12" cy="12" r="4" />
                    <path
                        d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M4.93 19.07l1.41-1.41M17.66 6.34l1.41-1.41"
                    />
                </svg>
            {:else}
                <!-- Moon: currently light, click for dark. -->
                <svg
                    xmlns="http://www.w3.org/2000/svg"
                    width="16"
                    height="16"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    aria-hidden="true"
                >
                    <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />
                </svg>
            {/if}
        </button>
    </nav>
    <div class="content">
        {@render children?.()}
    </div>
</div>

<style>
    /* Shared color tokens. The dark palette uses true black for the
       page background on purpose — the user's OLED panel switches
       those pixels off entirely, which is both easier on the eyes
       and lower-power. Surface tones are close to but not black so
       cards and tables still read as lifted. */
    :global(:root) {
        /* Tell the browser which palette to render native UA
           controls in (<select> dropdown, scrollbars, date
           pickers). Without this the dropdown list paints from the
           system light theme even when ludex is in dark mode. */
        color-scheme: light;
        --bg-page: #f7f7f9;
        --bg-surface: #ffffff;
        --bg-nav: #ffffff;
        --bg-hover: #f9fafb;
        --border: #e5e7eb;
        --border-strong: #9ca3af;
        --border-soft: #f3f4f6;
        --text-primary: #111111;
        --text-secondary: #333333;
        --text-body: #1a1a1a;
        --text-muted: #6b7280;
        --text-subtle: #9ca3af;
        --text-label: #374151;
        --accent: #1e40af;
        --status-open: #059669;
        --button-bg: #ffffff;
        --button-border: #d1d5db;
        --button-text: #333333;
        --button-hover-bg: #f4f5f7;
        --active-bg: #e5e7eb;
        --code-bg: #eceef2;
        --code-text: inherit;
        --error-bg: #fef2f2;
        --error-border: #fecaca;
        --error-text: #991b1b;
        --empty-border: #d1d5db;
        --row-shadow: rgba(0, 0, 0, 0.04);
    }

    :global(:root[data-theme='dark']) {
        color-scheme: dark;
        --bg-page: #000000;
        --bg-surface: #0f0f10;
        --bg-nav: #0a0a0b;
        --bg-hover: #18181b;
        --border: #27272a;
        --border-strong: #52525b;
        --border-soft: #1a1a1d;
        --text-primary: #fafafa;
        --text-secondary: #e4e4e7;
        --text-body: #e4e4e7;
        --text-muted: #a1a1aa;
        --text-subtle: #71717a;
        --text-label: #d4d4d8;
        --accent: #60a5fa;
        --status-open: #34d399;
        --button-bg: #18181b;
        --button-border: #3f3f46;
        --button-text: #e4e4e7;
        --button-hover-bg: #27272a;
        --active-bg: #27272a;
        --code-bg: #18181b;
        --code-text: #e4e4e7;
        --error-bg: #1a0606;
        --error-border: #7f1d1d;
        --error-text: #fca5a5;
        --empty-border: #3f3f46;
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

    .theme-toggle {
        display: inline-flex;
        align-items: center;
        justify-content: center;
        padding: 0.35rem;
        width: 2rem;
        height: 2rem;
        color: var(--text-muted);
    }

    .theme-toggle:hover:not(:disabled) {
        color: var(--text-primary);
    }

    .content {
        flex: 1;
    }
</style>
