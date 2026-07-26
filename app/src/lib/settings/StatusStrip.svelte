<script lang="ts">
    import { onMount } from 'svelte';
    import { getVersion } from '@tauri-apps/api/app';
    import { openUrl } from '@tauri-apps/plugin-opener';
    import type { UnlistenFn } from '@tauri-apps/api/event';
    import {
        getGpuMemoryThresholdBytes,
        onDaemonDisconnected,
        onDaemonReconnected,
    } from '$lib/api';
    import SettingsCard from './SettingsCard.svelte';

    interface Props {
        onerror?: (message: string) => void;
    }
    let { onerror }: Props = $props();

    /** Public repo URL — kept in sync with `repository` in the
     *  workspace Cargo.toml. */
    const REPO_URL = 'https://github.com/embernode/ludex';

    /** Resolved on mount via Tauri's `getVersion()`; empty until the
     *  promise resolves, and the segment is simply omitted if an
     *  older Tauri lacks the API. */
    let appVersion = $state<string>('');

    /** `null` until the first probe settles, so the strip doesn't
     *  claim either state before it knows. */
    let connected = $state<boolean | null>(null);

    /**
     * Liveness probe. There is no dedicated ping on the wire, so this
     * reads a cheap setting: it round-trips through the same D-Bus
     * connection the events report on, which is exactly what the dot
     * is describing.
     */
    async function probe() {
        let alive: boolean;
        try {
            await getGpuMemoryThresholdBytes();
            alive = true;
        } catch (_) {
            // A failed probe is the signal, not an error to surface —
            // the cards already report their own load failures, and
            // two banners for one dead daemon is noise.
            alive = false;
        }
        // Only seed the initial state. A D-Bus call against a dead
        // daemon can take seconds to time out, and if the daemon came
        // up in the meantime a `daemon-reconnected` event has already
        // set this — letting the stale rejection land would pin the
        // dot to "unreachable" while every card on the page works.
        if (connected === null) connected = alive;
    }

    async function openRepo() {
        try {
            await openUrl(REPO_URL);
        } catch (e) {
            onerror?.(String(e));
        }
    }

    onMount(() => {
        getVersion()
            .then((v) => (appVersion = v))
            .catch(() => {});
        probe();
        const unlistens: Promise<UnlistenFn>[] = [
            onDaemonDisconnected(() => (connected = false)),
            onDaemonReconnected(() => (connected = true)),
        ];
        return () => {
            for (const p of unlistens) {
                p.then((u) => u()).catch(() => {});
            }
        };
    });
</script>

<SettingsCard>
    <!-- Two tiers, because the strip carries two unlike things: one
         live indicator and a set of static build facts. Weighting them
         identically buried the only part that ever changes. -->
    <div class="live">
        <!-- Fixed brand green, deliberately not `var(--ac)`: this is a
             health indicator, and picking the lavender accent must not
             turn "daemon running" lavender. -->
        <span
            class="dot"
            class:down={connected === false}
            class:unknown={connected === null}
        ></span>
        <!-- Announced on change: this is the one string on the page
             that mutates on its own, from a daemon event rather than
             from anything the user just did. -->
        <span class="state" aria-live="polite">
            {#if connected === null}
                checking daemon
            {:else if connected}
                daemon running
            {:else}
                daemon unreachable
            {/if}
        </span>
        <button type="button" class="ghlink" onclick={openRepo}>github ↗</button>
    </div>

    <!-- Label/value pairs rather than one long sentence. Each pair is
         an atomic flex item, so the row folds at any width without a
         line ever beginning with a stranded separator — which is what
         the middots baked into the old markup could not avoid. -->
    <dl class="facts">
        {#if appVersion}
            <div class="fact">
                <dt>version</dt>
                <dd>{appVersion}</dd>
            </div>
        {/if}
        <div class="fact">
            <dt>licence</dt>
            <dd>MIT / Apache-2.0</dd>
        </div>
        <div class="fact">
            <dt>data</dt>
            <dd><code>$XDG_DATA_HOME/ludex/</code></dd>
        </div>
        <div class="fact">
            <dt>privacy</dt>
            <dd>no telemetry, no network egress</dd>
        </div>
    </dl>
</SettingsCard>

<style>
    .live {
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 12px 16px;
        border-bottom: 1px solid var(--line);
    }

    .dot {
        width: 6px;
        height: 6px;
        border-radius: 99px;
        background: var(--brand-green);
        flex: none;
    }

    .dot.down {
        background: var(--warn);
    }

    .dot.unknown {
        background: var(--fg3);
    }

    .state {
        font-family: 'JetBrains Mono', ui-monospace, monospace;
        font-size: 12px;
        color: var(--fg);
    }

    .facts {
        display: flex;
        flex-wrap: wrap;
        column-gap: 20px;
        row-gap: 6px;
        margin: 0;
        padding: 11px 16px 12px;
    }

    .fact {
        display: flex;
        align-items: baseline;
        gap: 6px;
        font-family: 'JetBrains Mono', ui-monospace, monospace;
        font-size: 11.5px;
    }

    .fact dt {
        color: var(--fg3);
    }

    .fact dd {
        margin: 0;
        color: var(--fg2);
    }

    .fact code {
        font-family: inherit;
        font-size: inherit;
        background: none;
        padding: 0;
        color: inherit;
    }

    .ghlink {
        font-family: 'JetBrains Mono', ui-monospace, monospace;
        font-size: 11.5px;
        color: var(--fg2);
        background: none;
        border: 0;
        border-bottom: 1px solid var(--line);
        border-radius: 0;
        padding: 0;
        cursor: pointer;
        flex: none;
        /* Takes the slack an empty spacer element used to hold. The
           tier does not wrap, so there is always a row to sit at the
           end of. */
        margin-left: auto;
    }

    .ghlink:hover {
        color: var(--fg);
        border-bottom-color: var(--fg);
        background: none;
    }
</style>
