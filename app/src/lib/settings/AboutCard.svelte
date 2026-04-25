<script lang="ts">
    import { onMount } from 'svelte';
    import { getVersion } from '@tauri-apps/api/app';
    import { openUrl } from '@tauri-apps/plugin-opener';

    interface Props {
        /** Bubbles up errors so the page renders one banner. */
        onerror?: (message: string) => void;
    }
    let { onerror }: Props = $props();

    /** Public repo URL — kept in sync with `repository` in the
     *  workspace Cargo.toml. */
    const REPO_URL = 'https://github.com/embernode/ludex';

    /** Resolved on mount via Tauri's `getVersion()`; empty until the
     *  promise resolves. An older Tauri without the API leaves it
     *  blank rather than failing — the dash placeholder takes over. */
    let appVersion = $state<string>('');

    onMount(() => {
        getVersion()
            .then((v) => {
                appVersion = v;
            })
            .catch(() => {});
    });

    async function openRepo() {
        try {
            await openUrl(REPO_URL);
        } catch (e) {
            onerror?.(String(e));
        }
    }
</script>

<section class="settings-card about">
    <h2>About</h2>
    <p class="about-tagline">Linux gameplay time tracker.</p>
    <dl class="about-facts">
        <dt>Version</dt>
        <dd>{appVersion || '—'}</dd>
        <dt>License</dt>
        <dd>Dual MIT / Apache-2.0</dd>
        <dt>Repository</dt>
        <dd>
            <button type="button" class="link-button" onclick={openRepo}>
                {REPO_URL}
            </button>
        </dd>
    </dl>
    <p class="about-privacy">
        No telemetry. No network egress. Data stays under
        <code>$XDG_DATA_HOME/ludex/</code>.
    </p>
</section>

<style>
    .about h2 {
        margin-bottom: 0.75rem;
    }

    .about-tagline {
        color: var(--text-secondary);
        margin: 0 0 1rem;
    }

    .about-facts {
        display: grid;
        grid-template-columns: max-content 1fr;
        gap: 0.25rem 1rem;
        margin: 0 0 1rem;
        font-size: 0.88rem;
    }

    .about-facts dt {
        color: var(--text-subtle);
        text-transform: uppercase;
        font-size: 0.75rem;
        letter-spacing: 0.03em;
        align-self: center;
    }

    .about-facts dd {
        margin: 0;
        color: var(--text-secondary);
    }

    .about-privacy {
        color: var(--text-muted);
        font-size: 0.82rem;
        margin: 0;
        line-height: 1.5;
    }

    .about-privacy code {
        font-family: 'JetBrains Mono', ui-monospace, monospace;
        background: var(--code-bg);
        color: var(--code-text);
        padding: 0.1rem 0.35rem;
        border-radius: 4px;
        font-size: 0.8rem;
    }
</style>
