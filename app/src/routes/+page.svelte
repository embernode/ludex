<script lang="ts">
    import { onMount } from 'svelte';
    import type { UnlistenFn } from '@tauri-apps/api/event';
    import {
        listApplications,
        listBlockedApplicationIds,
        onApplicationAdded,
        onBlocklistChanged,
        onDaemonReconnected,
        onSessionEnded,
        onSessionStarted,
        type ApplicationSummary,
    } from '$lib/api';
    import {
        formatSeconds,
        formatTimestamp,
        interactiveShare,
        observeTimestampFormat,
        type TimestampFormat,
    } from '$lib/format';
    import MonogramTile from '$lib/MonogramTile.svelte';
    import SourceLabel from '$lib/SourceLabel.svelte';

    let apps = $state<ApplicationSummary[]>([]);
    let hiddenBlocked = $state(0);
    let loading = $state(true);
    let error = $state<string | null>(null);
    let tsFormat = $state<TimestampFormat>('short');

    /** Case-insensitive filter on product name + publisher. */
    let filterQuery = $state<string>('');
    /**
     * Which field drives the order. `recent` matches the daemon's
     * default ORDER BY and is the default here too; `name` is
     * alphabetical; `played` is total full-runtime, most to least.
     */
    type SortKey = 'recent' | 'name' | 'played';
    let sortBy = $state<SortKey>('recent');

    const visibleApps = $derived.by(() => {
        const q = filterQuery.trim().toLowerCase();
        const filtered = q
            ? apps.filter(
                  (a) =>
                      a.product_name.toLowerCase().includes(q) ||
                      (a.publisher && a.publisher.toLowerCase().includes(q)),
              )
            : apps;
        const sorted = [...filtered];
        switch (sortBy) {
            case 'name':
                sorted.sort((a, b) =>
                    a.product_name.localeCompare(b.product_name, undefined, {
                        sensitivity: 'base',
                    }),
                );
                break;
            case 'played':
                sorted.sort(
                    (a, b) => b.total_full_seconds - a.total_full_seconds,
                );
                break;
            case 'recent':
            default:
                // Empty `last_played_at` (never played) sinks to
                // the bottom — matches the daemon's NULLS LAST.
                sorted.sort((a, b) => {
                    const at = a.last_played_at || '';
                    const bt = b.last_played_at || '';
                    if (!at && !bt) return 0;
                    if (!at) return 1;
                    if (!bt) return -1;
                    return bt.localeCompare(at);
                });
                break;
        }
        return sorted;
    });

    /** Doubles as a result counter while a filter is active. */
    const subCount = $derived.by(() => {
        const q = filterQuery.trim();
        if (q) {
            const n = visibleApps.length;
            return `${n} ${n === 1 ? 'match' : 'matches'}`;
        }
        const sessions = apps.reduce((sum, a) => sum + a.run_count, 0);
        return (
            `${apps.length} ${apps.length === 1 ? 'game' : 'games'} · ` +
            `${sessions} ${sessions === 1 ? 'session' : 'sessions'}`
        );
    });

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
        const unobserveTs = observeTimestampFormat((f) => (tsFormat = f));
        const unlisteners: Promise<UnlistenFn>[] = [
            onApplicationAdded(refresh),
            onSessionStarted(refresh),
            onSessionEnded(refresh),
            onDaemonReconnected(refresh),
            onBlocklistChanged(refresh),
        ];
        return () => {
            unobserveTs();
            for (const p of unlisteners) {
                p.then((unlisten) => unlisten()).catch(() => {});
            }
        };
    });
</script>

<main>
    <div class="titlerow">
        <h1>Library</h1>
        <!-- Counts are a claim about the user's data, so they wait for
             a fetch that actually succeeded — "0 games · 0 sessions"
             above "Couldn't reach the daemon" is a lie. -->
        {#if !loading && !error}
            <span class="subcount">{subCount}</span>
        {/if}
        <div class="spacer"></div>
        {#if !loading && !error && apps.length > 0}
            <div class="controls">
                <label>
                    <span class="visually-hidden">Filter games</span>
                    <input
                        type="search"
                        placeholder="Filter by name or publisher…"
                        bind:value={filterQuery}
                    />
                </label>
                <label>
                    <span class="visually-hidden">Sort games</span>
                    <select bind:value={sortBy}>
                        <option value="recent">Last played ↓</option>
                        <option value="name">Name A–Z</option>
                        <option value="played">Total runtime ↓</option>
                    </select>
                </label>
            </div>
        {/if}
    </div>

    {#if loading && apps.length === 0}
        <p class="state">Loading…</p>
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
                Launch a game through Steam or Proton while the daemon is
                running, or open any fullscreen game with a graphics library
                loaded — it will appear here automatically.
            </p>
        </div>
    {:else if apps.length === 0}
        <div class="empty">
            <p>Nothing to show — every tracked game is blocked.</p>
            <p class="hint">
                Unblock from <a href="/settings/detections">Detections</a> to see
                them here again.
            </p>
        </div>
    {:else if visibleApps.length === 0}
        <p class="state">No games match "{filterQuery}".</p>
    {:else}
        <div class="tablecard">
            <div class="grid thead" aria-hidden="true">
                <span></span>
                <span>GAME</span>
                <span class="right">RUNS</span>
                <span class="right">FULL</span>
                <span>INTERACTIVE</span>
                <span>DETECTED VIA</span>
                <span class="right">LAST PLAYED</span>
        </div>
        <ul class="rows">
            {#each visibleApps as app (app.id)}
                <li>
                    <a class="grid row" href="/app/{app.id}">
                        <MonogramTile name={app.product_name} />
                        <span class="gname">{app.product_name}</span>
                        <span class="right num dim">{app.run_count}</span>
                        <span class="right num strong">
                            {formatSeconds(app.total_full_seconds)}
                        </span>
                        <span class="interactive">
                            <span class="bar">
                                <span
                                    style="width:{interactiveShare(
                                        app.total_interactive_seconds,
                                        app.total_full_seconds,
                                    )}%"
                                ></span>
                            </span>
                            <span class="mono num dim">
                                {formatSeconds(app.total_interactive_seconds)}
                            </span>
                        </span>
                        <SourceLabel launcherType={app.launcher_type} />
                        <span class="right num dim">
                            {formatTimestamp(app.last_played_at, tsFormat)}
                        </span>
                    </a>
                </li>
            {/each}
        </ul>
        </div>
    {/if}
</main>

<style>
    main {
        max-width: 1000px;
        margin: 0 auto;
        padding: 22px 20px 40px;
    }

    .titlerow {
        display: flex;
        align-items: flex-end;
        gap: 14px;
        margin-bottom: 16px;
    }

    h1 {
        font-size: 24px;
        font-weight: 600;
        line-height: 1;
        margin: 0;
        letter-spacing: -0.02em;
    }

    .subcount {
        font-size: 13px;
        padding-bottom: 2px;
        color: var(--fg3);
    }

    .spacer {
        flex: 1;
    }

    .controls {
        display: flex;
        align-items: center;
        gap: 8px;
        padding-bottom: 1px;
    }

    .controls input {
        width: 180px;
        font: inherit;
        font-size: 12.5px;
        color: var(--fg);
        background: var(--surface);
        border: 1px solid var(--line);
        border-radius: 6px;
        padding: 5px 9px;
        outline: none;
    }

    .controls input:focus {
        border-color: var(--ac);
    }

    /* Bare by design — the control reads as a label that happens to be
       clickable rather than as a form field. */
    .controls select {
        appearance: none;
        -webkit-appearance: none;
        font: inherit;
        font-size: 12.5px;
        color: var(--fg2);
        background: transparent;
        border: none;
        border-radius: 6px;
        padding: 5px 4px;
        cursor: pointer;
        outline: none;
    }

    .controls select:focus-visible {
        outline: 2px solid var(--ac);
        outline-offset: 1px;
    }

    .grid {
        display: grid;
        grid-template-columns:
            34px minmax(180px, 1fr) 54px 92px 168px 140px 112px;
        gap: 13px;
        align-items: center;
    }

    /* The table sits on its own surface, like the activity cards, so
       the page's side padding reads as a gap rather than as nothing —
       a bare table on the page background looks flush to the window
       edge even when the padding is there. */
    .tablecard {
        background: var(--surface);
        border: 1px solid var(--line);
        border-radius: 9px;
        overflow: hidden;
    }

    .thead {
        padding: 12px 13px 8px;
        border-bottom: 1px solid var(--line);
        font-size: 10.5px;
        font-weight: 500;
        letter-spacing: 0.09em;
        color: var(--fg3);
    }

    .rows {
        list-style: none;
        margin: 0;
        padding: 0;
    }

    .row {
        padding: 10px 13px;
        border-bottom: 1px solid var(--hair);
        text-decoration: none;
        color: inherit;
    }

    .rows li:last-child .row {
        border-bottom: 0;
    }

    /* The card clips to its padding box, so an outline drawn outside
       the row's border box would be cut off. Inset it. */
    .row:focus-visible {
        outline: 2px solid var(--ac);
        outline-offset: -2px;
    }

    .row:hover {
        background: var(--tile);
    }

    .gname {
        font-size: 13.5px;
        font-weight: 600;
        color: var(--fg);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    .right {
        text-align: right;
    }

    .num {
        font-variant-numeric: tabular-nums;
    }

    .mono {
        font-family: 'JetBrains Mono', ui-monospace, monospace;
        font-size: 12px;
    }

    .dim {
        font-size: 12px;
        color: var(--fg2);
    }

    .strong {
        font-size: 13px;
        font-weight: 500;
    }

    .interactive {
        display: flex;
        align-items: center;
        gap: 9px;
        min-width: 0;
    }

    .bar {
        flex: 1;
        height: 5px;
        border-radius: 99px;
        overflow: hidden;
        background: var(--track);
    }

    .bar > span {
        display: block;
        height: 100%;
        background: var(--ac);
    }

    .state {
        font-size: 12.5px;
        color: var(--fg3);
        padding: 20px 13px;
        margin: 0;
    }
</style>
