<script lang="ts">
    import { onMount } from 'svelte';
    import type { UnlistenFn } from '@tauri-apps/api/event';
    import {
        listApplications,
        listBlockedApplicationIds,
        onApplicationAdded,
        onSessionEnded,
        onSessionStarted,
        type ApplicationSummary,
    } from '$lib/api';
    import { formatSeconds, formatTimestamp } from '$lib/format';

    let apps = $state<ApplicationSummary[]>([]);
    let hiddenBlocked = $state(0);
    let loading = $state(true);
    let error = $state<string | null>(null);

    async function refresh() {
        try {
            const [allApps, blockedIds] = await Promise.all([
                listApplications(),
                // `.catch` so an older daemon without the M6.6.3
                // D-Bus methods degrades to "nothing blocked"
                // instead of hiding the apps page entirely.
                listBlockedApplicationIds().catch(() => [] as number[]),
            ]);
            const blocked = new Set(blockedIds);
            apps = allApps.filter((a) => !blocked.has(a.id));
            hiddenBlocked = allApps.length - apps.length;
            error = null;
        } catch (e) {
            error = String(e);
        } finally {
            loading = false;
        }
    }

    onMount(() => {
        refresh();
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
        <h1>Games</h1>
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
    {:else if apps.length === 0 && hiddenBlocked === 0}
        <div class="empty">
            <p>No games tracked yet.</p>
            <p class="hint">
                Launch a game through Steam or Proton while the daemon is running,
                or open any fullscreen game with a graphics library loaded — it
                will appear here automatically.
            </p>
        </div>
    {:else if apps.length === 0}
        <div class="empty">
            <p>Nothing to show — every tracked game is blocked.</p>
            <p class="hint">
                Unblock from <a href="/settings">Settings</a> to see them here
                again.
            </p>
        </div>
    {:else}
        <ul class="apps">
            {#each apps as app (app.id)}
                <li>
                    <a class="row-link" href="/app/{app.id}">
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
                                    >{formatSeconds(
                                        app.total_interactive_seconds,
                                    )}</span
                                >
                            </span>
                            <span class="stat">
                                <span class="stat-label">last played</span>
                                <span class="stat-value"
                                    >{formatTimestamp(app.last_played_at)}</span
                                >
                            </span>
                        </div>
                    </a>
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
        margin-bottom: 1.5rem;
    }

    h1 {
        font-size: 1.75rem;
        font-weight: 600;
        margin: 0;
        letter-spacing: -0.02em;
    }

    .apps {
        list-style: none;
        padding: 0;
        margin: 0;
    }

    .apps li {
        margin-bottom: 0.5rem;
    }

    .row-link {
        display: block;
        border: 1px solid var(--border);
        border-radius: 8px;
        padding: 1rem 1.25rem;
        background: var(--bg-surface);
        color: inherit;
        text-decoration: none;
        transition:
            border-color 120ms,
            box-shadow 120ms;
    }

    .row-link:hover {
        border-color: var(--border-strong);
        box-shadow: 0 1px 3px var(--row-shadow);
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
        color: var(--text-primary);
    }

    .publisher {
        font-size: 0.85rem;
        color: var(--text-muted);
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
        color: var(--text-subtle);
        font-size: 0.75rem;
        text-transform: uppercase;
        letter-spacing: 0.03em;
    }

    .stat-value {
        color: var(--text-secondary);
        font-variant-numeric: tabular-nums;
    }
</style>
