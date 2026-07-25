<script lang="ts">
    import { onMount } from 'svelte';
    import type { UnlistenFn } from '@tauri-apps/api/event';
    import {
        listApplications,
        listBlockedApplicationIds,
        onApplicationAdded,
        onBlocklistChanged,
        onDaemonReconnected,
    } from '$lib/api';
    import { isNewSince, storedWatermark } from './detections';

    interface Props {
        onerror?: (message: string) => void;
    }
    let { onerror }: Props = $props();

    let total = $state<number>(0);
    let blockedCount = $state<number>(0);
    let newCount = $state<number>(0);

    /** The counts are a claim about the user's data, so they stay
     *  hidden until a fetch has actually produced them — otherwise a
     *  daemon-down page reads "0 executables the gate has accepted"
     *  directly under an error banner. */
    let loaded = $state<boolean>(false);

    async function load() {
        try {
            const [apps, ids] = await Promise.all([
                listApplications(),
                listBlockedApplicationIds(),
            ]);
            total = apps.length;
            blockedCount = ids.length;
            const watermark = storedWatermark();
            newCount = apps.filter((a) =>
                isNewSince(a.first_seen_at, watermark),
            ).length;
            loaded = true;
        } catch (e) {
            loaded = false;
            onerror?.(String(e));
        }
    }

    onMount(() => {
        load();
        const unlistens: Promise<UnlistenFn>[] = [
            onDaemonReconnected(load),
            onBlocklistChanged(load),
            // A newly detected game changes both the total and the
            // "N new" hint this row exists to surface.
            onApplicationAdded(load),
        ];
        return () => {
            for (const p of unlistens) {
                p.then((u) => u()).catch(() => {});
            }
        };
    });
</script>

<a class="card linkrow" href="/settings/detections">
    <div>
        <div class="cardtitle">Detections</div>
        <div class="cardsub">
            {#if loaded}
                {total}
                {total === 1 ? 'executable' : 'executables'} the gate has
                accepted · {blockedCount} blocked{#if newCount > 0}
                    · <span class="new">{newCount} new</span>
                {/if}
            {:else}
                Everything the gate has accepted, with block and allow.
            {/if}
        </div>
    </div>
    <span class="chevron">→</span>
</a>

<style>
    .card {
        background: var(--surface);
        border: 1px solid var(--line);
        border-radius: 9px;
        margin-bottom: 14px;
        text-decoration: none;
        color: inherit;
    }

    .linkrow {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 14px;
        padding: 13px 16px;
    }

    .linkrow:hover {
        background: var(--tile);
    }

    .cardtitle {
        font-size: 13.5px;
        font-weight: 600;
        color: var(--fg);
    }

    .cardsub {
        font-size: 11.5px;
        margin-top: 2px;
        color: var(--fg3);
    }

    .new {
        color: var(--ac);
    }

    .chevron {
        color: var(--fg3);
        flex: none;
    }
</style>
