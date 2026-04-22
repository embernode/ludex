<script lang="ts">
    import { onMount } from 'svelte';
    import type { UnlistenFn } from '@tauri-apps/api/event';
    import {
        listApplications,
        onApplicationAdded,
        onSessionEnded,
        onSessionStarted,
        type ApplicationSummary,
    } from '$lib/api';
    import { formatSeconds, formatTimestamp } from '$lib/format';

    // Reactive state. Svelte 5 runes: `$state` makes a value
    // reactive; `$derived` computes a view of it.
    let apps = $state<ApplicationSummary[]>([]);
    let loading = $state(true);
    let error = $state<string | null>(null);

    async function refresh() {
        try {
            apps = await listApplications();
            error = null;
        } catch (e) {
            error = String(e);
        } finally {
            loading = false;
        }
    }

    onMount(() => {
        refresh();

        // Subscribe to daemon signals so the UI reflects live
        // changes without polling. `listen` returns a promise that
        // resolves once the match rule is registered; the resolved
        // function unsubscribes.
        const unlisteners: Promise<UnlistenFn>[] = [
            onApplicationAdded(refresh),
            onSessionStarted(refresh),
            onSessionEnded(refresh),
        ];

        return () => {
            for (const p of unlisteners) {
                p.then((unlisten) => unlisten()).catch(() => {});
            }
        };
    });
</script>

<main>
    <header>
        <div class="title">
            <h1>ludex</h1>
            <span class="tag">pre-alpha</span>
        </div>
        <button onclick={refresh} disabled={loading}>Refresh</button>
    </header>

    {#if loading && apps.length === 0}
        <p class="hint">Loading…</p>
    {:else if error}
        <div class="error">
            <p><strong>Couldn't reach the daemon.</strong></p>
            <p class="detail">{error}</p>
            <p class="hint">Is <code>ludex-daemon</code> running?</p>
        </div>
    {:else if apps.length === 0}
        <div class="empty">
            <p>No games tracked yet.</p>
            <p class="hint">
                Launch a game through Steam or Proton while the daemon is running,
                or open any fullscreen game with a graphics library loaded — it
                will appear here automatically.
            </p>
        </div>
    {:else}
        <ul class="apps">
            {#each apps as app (app.id)}
                <li>
                    <div class="name">
                        <span class="product">{app.product_name}</span>
                        {#if app.publisher}
                            <span class="publisher">{app.publisher}</span>
                        {/if}
                    </div>
                    <div class="stats">
                        <span class="stat">
                            <span class="stat-label">runs</span>
                            <span class="stat-value">{app.run_count}</span>
                        </span>
                        <span class="stat">
                            <span class="stat-label">full</span>
                            <span class="stat-value"
                                >{formatSeconds(app.total_full_seconds)}</span
                            >
                        </span>
                        <span class="stat">
                            <span class="stat-label">interactive</span>
                            <span class="stat-value"
                                >{formatSeconds(app.total_interactive_seconds)}</span
                            >
                        </span>
                        <span class="stat">
                            <span class="stat-label">last played</span>
                            <span class="stat-value"
                                >{formatTimestamp(app.last_played_at)}</span
                            >
                        </span>
                    </div>
                </li>
            {/each}
        </ul>
    {/if}
</main>

<style>
    main {
        max-width: 72ch;
        margin: 0 auto;
        padding: 2rem;
    }

    header {
        display: flex;
        justify-content: space-between;
        align-items: center;
        margin-bottom: 2rem;
    }

    .title {
        display: flex;
        align-items: baseline;
        gap: 0.6rem;
    }

    h1 {
        font-size: 2.25rem;
        font-weight: 600;
        margin: 0;
        letter-spacing: -0.02em;
    }

    .tag {
        font-size: 0.75rem;
        padding: 0.1rem 0.45rem;
        border-radius: 999px;
        background: #e6e7eb;
        color: #666;
        font-weight: 500;
    }

    button {
        font: inherit;
        padding: 0.4rem 0.9rem;
        border: 1px solid #d1d5db;
        background: white;
        border-radius: 6px;
        cursor: pointer;
        color: #333;
    }

    button:hover:not(:disabled) {
        background: #f4f5f7;
    }

    button:disabled {
        opacity: 0.5;
        cursor: default;
    }

    .hint {
        color: #6b7280;
        font-size: 0.9rem;
    }

    .error {
        background: #fef2f2;
        border: 1px solid #fecaca;
        border-radius: 6px;
        padding: 1rem;
    }

    .error p {
        margin: 0.25rem 0;
    }

    .error .detail {
        font-family: 'JetBrains Mono', ui-monospace, monospace;
        font-size: 0.85rem;
        color: #991b1b;
    }

    .empty {
        border: 1px dashed #d1d5db;
        border-radius: 6px;
        padding: 1.5rem;
        text-align: center;
    }

    .empty p:first-child {
        font-size: 1.1rem;
        color: #444;
    }

    .apps {
        list-style: none;
        padding: 0;
        margin: 0;
    }

    .apps li {
        border: 1px solid #e5e7eb;
        border-radius: 8px;
        padding: 1rem 1.25rem;
        margin-bottom: 0.5rem;
        background: white;
        transition: border-color 120ms;
    }

    .apps li:hover {
        border-color: #c7cad1;
    }

    .name {
        display: flex;
        align-items: baseline;
        gap: 0.75rem;
        margin-bottom: 0.5rem;
    }

    .product {
        font-size: 1.05rem;
        font-weight: 600;
        color: #111;
    }

    .publisher {
        font-size: 0.85rem;
        color: #6b7280;
    }

    .stats {
        display: flex;
        flex-wrap: wrap;
        gap: 1.25rem;
        font-size: 0.85rem;
    }

    .stat {
        display: flex;
        flex-direction: column;
    }

    .stat-label {
        color: #9ca3af;
        font-size: 0.75rem;
        text-transform: uppercase;
        letter-spacing: 0.03em;
    }

    .stat-value {
        color: #333;
        font-variant-numeric: tabular-nums;
    }

    code {
        font-family: 'JetBrains Mono', ui-monospace, monospace;
        font-size: 0.9em;
        background: #eceef2;
        padding: 0.05em 0.35em;
        border-radius: 4px;
    }
</style>
