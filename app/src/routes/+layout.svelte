<script lang="ts">
    import { page } from '$app/state';

    let { children } = $props();

    // Mark a nav link active when the current route matches.
    // `startsWith` rather than `===` so nested routes (e.g.
    // `/app/42`) keep the `Apps` link active.
    function isActive(path: string): boolean {
        if (path === '/') return page.url.pathname === '/' || page.url.pathname.startsWith('/app/');
        return page.url.pathname === path || page.url.pathname.startsWith(`${path}/`);
    }
</script>

<div class="app">
    <nav>
        <a class="brand" href="/">ludex</a>
        <div class="links">
            <a href="/" class:active={isActive('/')}>Apps</a>
            <a href="/recent" class:active={isActive('/recent')}>Recent</a>
        </div>
        <span class="tag">pre-alpha</span>
    </nav>
    <div class="content">
        {@render children?.()}
    </div>
</div>

<style>
    :global(html, body) {
        margin: 0;
        padding: 0;
        height: 100%;
        font-family: system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif;
        background: #f7f7f9;
        color: #1a1a1a;
    }

    :global(button) {
        font: inherit;
        padding: 0.4rem 0.9rem;
        border: 1px solid #d1d5db;
        background: white;
        border-radius: 6px;
        cursor: pointer;
        color: #333;
    }

    :global(button:hover:not(:disabled)) {
        background: #f4f5f7;
    }

    :global(button:disabled) {
        opacity: 0.5;
        cursor: default;
    }

    :global(code) {
        font-family: 'JetBrains Mono', ui-monospace, monospace;
        font-size: 0.9em;
        background: #eceef2;
        padding: 0.05em 0.35em;
        border-radius: 4px;
    }

    :global(.hint) {
        color: #6b7280;
        font-size: 0.9rem;
    }

    :global(.error) {
        background: #fef2f2;
        border: 1px solid #fecaca;
        border-radius: 6px;
        padding: 1rem;
    }

    :global(.error p) {
        margin: 0.25rem 0;
    }

    :global(.error .detail) {
        font-family: 'JetBrains Mono', ui-monospace, monospace;
        font-size: 0.85rem;
        color: #991b1b;
    }

    :global(.empty) {
        border: 1px dashed #d1d5db;
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
        border-bottom: 1px solid #e5e7eb;
        background: white;
    }

    .brand {
        font-size: 1.15rem;
        font-weight: 600;
        color: #111;
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
        color: #4b5563;
        text-decoration: none;
        font-size: 0.95rem;
        transition: background 120ms, color 120ms;
    }

    .links a:hover {
        background: #f3f4f6;
        color: #111;
    }

    .links a.active {
        background: #e5e7eb;
        color: #111;
        font-weight: 500;
    }

    .tag {
        font-size: 0.75rem;
        padding: 0.1rem 0.45rem;
        border-radius: 999px;
        background: #e6e7eb;
        color: #666;
        font-weight: 500;
    }

    .content {
        flex: 1;
    }
</style>
