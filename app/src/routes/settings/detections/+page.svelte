<script lang="ts">
    import { onMount } from 'svelte';
    import type { UnlistenFn } from '@tauri-apps/api/event';
    import {
        blockApplication,
        listApplications,
        listBlockedApplicationIds,
        onApplicationAdded,
        onBlocklistChanged,
        onDaemonReconnected,
        unblockApplication,
        type ApplicationSummary,
    } from '$lib/api';
    import SourceLabel from '$lib/SourceLabel.svelte';
    import {
        isNewSince,
        stampWatermark,
        storedWatermark,
    } from '$lib/settings/detections';

    let apps = $state<ApplicationSummary[]>([]);
    let blocked = $state<Set<number>>(new Set());
    let filter = $state<'all' | 'blocked'>('all');
    let error = $state<string | null>(null);
    let loading = $state<boolean>(true);

    /** Counts and empty states may only assert once a fetch has
     *  actually succeeded — "All 0" next to "daemon unreachable" is a
     *  claim about the user's data that the app is in no position to
     *  make. */
    const loaded = $derived(!loading && error === null);

    /**
     * Read once on mount and then held for the life of the view, so
     * rows don't lose their badge underneath the user while they are
     * still looking at the list.
     */
    let watermark = $state<string | null>(null);

    /**
     * Whether the baseline has been advanced this visit. Advancing it
     * is deferred until the list has actually loaded: the watermark is
     * the only record of what the user has seen, so burning it on a
     * visit that rendered nothing — daemon down, or a mis-click
     * straight back out — would clear every badge with no way to get
     * them back.
     */
    let stamped = false;

    const visible = $derived(
        filter === 'blocked' ? apps.filter((a) => blocked.has(a.id)) : apps,
    );

    async function load() {
        try {
            const [a, ids] = await Promise.all([
                listApplications(),
                listBlockedApplicationIds(),
            ]);
            apps = a;
            blocked = new Set(ids);
            error = null;
            if (!stamped) {
                stampWatermark(new Date().toISOString());
                stamped = true;
            }
        } catch (e) {
            error = String(e);
        } finally {
            loading = false;
        }
    }

    async function toggleBlock(id: number) {
        try {
            if (blocked.has(id)) {
                await unblockApplication(id);
                blocked.delete(id);
            } else {
                await blockApplication(id);
                blocked.add(id);
            }
            // Reassign so Svelte sees the Set as a new value.
            blocked = new Set(blocked);
            error = null;
        } catch (e) {
            error = String(e);
        }
    }

    onMount(() => {
        watermark = storedWatermark();
        load();
        const unlistens: Promise<UnlistenFn>[] = [
            onDaemonReconnected(load),
            onBlocklistChanged(load),
            // A game detected while this view is open is precisely the
            // row the NEW badge exists for, so don't make the user
            // reopen the page to see it.
            onApplicationAdded(load),
        ];
        return () => {
            for (const p of unlistens) {
                p.then((u) => u()).catch(() => {});
            }
        };
    });
</script>

<main>
    <a class="back" href="/settings">← Settings</a>

    <div class="titlerow">
        <h1>Detections</h1>
        <div class="spacer"></div>
        <div class="seg">
            <button
                type="button"
                aria-pressed={filter === 'all'}
                onclick={() => (filter = 'all')}
            >
                All{loaded ? ` ${apps.length}` : ''}
            </button>
            <button
                type="button"
                aria-pressed={filter === 'blocked'}
                onclick={() => (filter = 'blocked')}
            >
                Blocked{loaded ? ` ${blocked.size}` : ''}
            </button>
        </div>
    </div>

    <p class="note">
        Every executable the detection gate has accepted at least once. Block
        one and ludex stops tracking it from the next launch — sessions
        already recorded stay, and can be deleted from the game's own page.
    </p>

    {#if error}
        <div class="error inline">
            <p class="detail">{error}</p>
            <button
                type="button"
                class="link-button"
                onclick={() => (error = null)}
                aria-label="Dismiss"
            >
                Dismiss
            </button>
        </div>
    {/if}

    <div class="tablecard">
        <div class="thead">
            <span>EXECUTABLE</span>
            <span>DETECTED VIA</span>
            <span class="right">BLOCKED</span>
    </div>

    {#if loading}
        <p class="state">Loading…</p>
    {:else if error}
        <p class="state">The ledger couldn't be read — see above.</p>
    {:else if visible.length === 0}
        <p class="state">
            {filter === 'blocked'
                ? 'Nothing is blocked.'
                : 'No applications detected yet.'}
        </p>
    {:else}
        <ul class="rows">
            {#each visible as app (app.id)}
                {@const isBlocked = blocked.has(app.id)}
                <li class="row">
                    <div class="identity">
                        <div class="nameline">
                            <span class="name">{app.product_name}</span>
                            {#if isNewSince(app.first_seen_at, watermark)}
                                <span class="chip">NEW</span>
                            {/if}
                        </div>
                        <!-- Empty for Steam titles found through the
                             content log, which record no executable. -->
                        <div class="path" title={app.executable_path}>
                            {app.executable_path}
                        </div>
                    </div>
                    <SourceLabel
                        launcherType={app.launcher_type}
                        detectedVia={app.detected_via}
                    />
                    <button
                        type="button"
                        class="btn"
                        class:warn={!isBlocked}
                        aria-label={isBlocked
                            ? `Unblock ${app.product_name}`
                            : `Block ${app.product_name}`}
                        onclick={() => toggleBlock(app.id)}
                    >
                        {isBlocked ? 'Unblock' : 'Block'}
                    </button>
                </li>
            {/each}
        </ul>
    {/if}
    </div>
</main>

<style>
    main {
        max-width: 800px;
        margin: 0 auto;
        padding: 22px 20px 40px;
    }

    .back {
        display: inline-block;
        font-size: 12.5px;
        color: var(--fg3);
        text-decoration: none;
        margin-bottom: 16px;
    }

    .back:hover {
        color: var(--fg);
    }

    .titlerow {
        display: flex;
        align-items: center;
        gap: 14px;
    }

    h1 {
        font-size: 24px;
        font-weight: 600;
        line-height: 1;
        margin: 0;
        letter-spacing: -0.02em;
    }

    .spacer {
        flex: 1;
    }

    .seg {
        display: flex;
        gap: 3px;
        background: var(--surface);
        border: 1px solid var(--line);
        border-radius: 7px;
        padding: 3px;
    }

    .seg button {
        font-size: 12px;
        font-weight: 500;
        border-radius: 5px;
        padding: 4px 12px;
        border: 0;
        background: transparent;
        color: var(--fg2);
        cursor: pointer;
    }

    .seg button[aria-pressed='true'] {
        background: var(--ac);
        color: var(--bg);
    }

    .note {
        font-size: 11.5px;
        line-height: 1.55;
        color: var(--fg3);
        max-width: 700px;
        margin: 14px 0 16px;
    }

    .thead,
    .row {
        display: grid;
        grid-template-columns: 1fr 150px 108px;
        gap: 12px;
        align-items: center;
    }

    /* Matches the library: a bare table on the page background reads
       as flush to the window edge even with the page padding there. */
    .tablecard {
        background: var(--surface);
        border: 1px solid var(--line);
        border-radius: 9px;
        overflow: hidden;
    }

    .thead {
        padding: 12px 12px 8px;
        border-bottom: 1px solid var(--line);
        font-size: 10.5px;
        font-weight: 500;
        letter-spacing: 0.09em;
        color: var(--fg3);
    }

    .right {
        text-align: right;
    }

    .rows {
        list-style: none;
        margin: 0;
        padding: 0;
    }

    .row {
        padding: 10px 12px;
        border-bottom: 1px solid var(--hair);
    }

    .row:focus-within {
        outline: 2px solid var(--ac);
        outline-offset: -2px;
    }

    .rows li:last-child.row {
        border-bottom: 0;
    }

    .identity {
        min-width: 0;
    }

    .nameline {
        display: flex;
        align-items: baseline;
        gap: 8px;
    }

    .name {
        font-size: 13px;
        font-weight: 500;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    .path {
        font-family: 'JetBrains Mono', ui-monospace, monospace;
        font-size: 11px;
        color: var(--fg3);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
        /* Hold the line's height so rows with no recorded executable
           stay the same height as rows that have one. */
        min-height: 1.2em;
    }

    .chip {
        font-size: 9.5px;
        font-weight: 500;
        letter-spacing: 0.06em;
        color: var(--ac);
        border: 1px solid color-mix(in oklab, var(--ac) 42%, transparent);
        border-radius: 4px;
        padding: 1px 5px;
        flex: none;
    }

    .btn {
        font-size: 12px;
        font-weight: 500;
        border-radius: 6px;
        padding: 5px 11px;
        cursor: pointer;
        color: var(--fg);
        background: var(--tile);
        border: 1px solid var(--line);
        justify-self: end;
    }

    .btn.warn {
        color: var(--warn);
        border-color: color-mix(in oklab, var(--warn) 40%, transparent);
        background: transparent;
    }

    /* Deliberately not `.empty` — that is a global class carrying the
       pre-redesign dashed-box chrome, and a scoped override would
       inherit its border and centring. */
    .state {
        font-size: 12.5px;
        color: var(--fg3);
        padding: 20px 12px;
        margin: 0;
    }
</style>
