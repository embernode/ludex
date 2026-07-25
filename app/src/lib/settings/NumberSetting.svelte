<script lang="ts">
    import SettingRow from './SettingRow.svelte';
    import { parseSetting, type Bounds } from './commit';

    interface Props {
        label: string;
        help?: string;
        /** Unit shown after the field ("s", "h", "MiB", "snapshots"). */
        unit?: string;
        bounds: Bounds;
        /** Currently persisted value. */
        value: number;
        /** Disable the field (e.g. a dependent setting is off). */
        disabled?: boolean;
        /**
         * Persist `next`. Rejecting via a thrown error leaves the
         * field showing the old value and surfaces the message, so
         * a daemon failure can't look like a successful save.
         */
        commit: (next: number) => Promise<void>;
    }
    let {
        label,
        help,
        unit,
        bounds,
        value,
        disabled = false,
        commit,
    }: Props = $props();

    /**
     * Uncommitted keystrokes, or `null` when the field is showing
     * what is actually stored. Keeping the edit separate from the
     * value means the field re-seeds itself whenever `value` changes
     * underneath us — the cards reload on daemon reconnect, and a
     * stale draft would silently disagree with what is persisted —
     * and reverting a rejected entry is just clearing this back to
     * `null`.
     */
    let editing = $state<string | null>(null);
    let error = $state<string | null>(null);
    let field = $state<HTMLInputElement | null>(null);

    const draft = $derived(editing ?? String(value));
    const errorId = $props.id();

    /**
     * Replay the commit flash. Done imperatively on the element: the
     * mockup's `classList.remove` / reflow / `add` works because those
     * DOM writes are synchronous, whereas toggling a piece of Svelte
     * state would batch both changes into one update and never restart
     * the animation.
     */
    function flash() {
        const el = field;
        if (!el) return;
        el.classList.remove('flash');
        void el.offsetWidth;
        el.classList.add('flash');
    }

    async function onCommit() {
        const attempted = draft;
        const outcome = parseSetting(attempted, bounds, value);
        if (outcome.status === 'unchanged') {
            editing = null;
            error = null;
            return;
        }
        if (outcome.status === 'invalid') {
            error = outcome.message;
            editing = null;
            return;
        }
        error = null;
        try {
            await commit(outcome.value);
            // Don't clobber anything typed while the round-trip was in
            // flight — only clear the draft if it's still the value we
            // committed.
            if (editing === attempted) editing = null;
            flash();
        } catch (e) {
            error = String(e);
            if (editing === attempted) editing = null;
        }
    }

    /**
     * Drop an uncommitted draft on the way out. Without this, a field
     * the user touched and then abandoned (type a character, delete
     * it — no `change` event fires) keeps `editing` pinned to a
     * string, and from then on ignores every reload the cards do on
     * daemon reconnect.
     */
    function onBlur() {
        editing = null;
    }
</script>

<SettingRow {label} {help} {error} {errorId}>
    {#snippet control()}
        <input
            bind:this={field}
            class="numfield"
            type="text"
            inputmode="numeric"
            {disabled}
            aria-label={label}
            aria-invalid={error ? 'true' : undefined}
            aria-describedby={error ? errorId : undefined}
            value={draft}
            oninput={(e) => {
                editing = e.currentTarget.value;
                // The message described a value that is no longer on
                // screen; leaving it up would flag a field the user
                // has already corrected.
                error = null;
            }}
            onchange={onCommit}
            onblur={onBlur}
        />
        <span class="unit">{unit ?? ''}</span>
    {/snippet}
</SettingRow>

<style>
    .numfield {
        font-family: 'JetBrains Mono', ui-monospace, Menlo, monospace;
        font-size: 13px;
        color: var(--fg);
        background: var(--bg);
        border: 1px solid var(--line);
        border-radius: 6px;
        padding: 6px 10px;
        width: 74px;
        text-align: right;
        font-variant-numeric: tabular-nums;
    }

    .numfield:focus {
        outline: none;
        border-color: var(--ac);
    }

    .numfield[aria-invalid='true'] {
        border-color: var(--error-text);
    }

    .numfield:disabled {
        opacity: 0.55;
        cursor: not-allowed;
    }

    /* Fixed width, left-aligned, and rendered even when there is no
       unit: the control block is right-aligned in the row, so a
       variable-width suffix ("snapshots" vs "h") would push the input
       to a different x on every row. Sized to the longest unit so all
       the fields line up down the page. */
    .unit {
        font-size: 11.5px;
        color: var(--fg3);
        flex: none;
        width: 62px;
        text-align: left;
    }

    /* Instant apply gives no other confirmation that a value landed.
       `.flash` is added imperatively — see flash() — so it must not be
       scoped away by the compiler. */
    :global(.numfield.flash) {
        animation: flash 0.5s ease-out;
    }

    @keyframes flash {
        from {
            background: var(--pill-bg);
        }
        to {
            background: var(--bg);
        }
    }

    @media (prefers-reduced-motion: reduce) {
        :global(.numfield.flash) {
            animation: none;
        }
    }
</style>
