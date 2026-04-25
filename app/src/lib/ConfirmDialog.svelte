<script lang="ts">
    import type { Snippet } from 'svelte';

    /**
     * Modal confirmation dialog backed by the native `<dialog>`
     * element. The native flavour gives us focus trapping, ESC
     * dismissal, and a `::backdrop` pseudo-element for free —
     * none of which `window.confirm` does inside a webview, and
     * which in our case also leaks a `http://localhost:1420/...`
     * title bar.
     *
     * The parent owns the `dialog` ref via `bind:dialog` and calls
     * `dialog.showModal()` / `dialog.close()` imperatively. That
     * keeps the API minimal and avoids re-implementing the open
     * state machine in Svelte. The component owns the markup, the
     * `busy` flag for the in-flight RPC, and the styling.
     */
    interface Props {
        /** Two-way binding so the parent owns the imperative API
         *  (`showModal()` / `close()`) without us re-exposing it. */
        dialog?: HTMLDialogElement | null;
        /** Heading rendered at the top of the dialog. */
        title: string;
        /** Body content; typically a fact list + a warning paragraph. */
        body?: Snippet;
        /** Label of the affirmative button (e.g. "Delete session"). */
        confirmLabel?: string;
        /** Label shown on the affirmative button while `onconfirm`
         *  is in flight. Both buttons are disabled in this state. */
        confirmBusyLabel?: string;
        /** Label of the dismissal button. */
        cancelLabel?: string;
        /** When true, the affirmative button gets destructive
         *  colouring (accent border + text → solid red on hover). */
        danger?: boolean;
        /** Called when the user confirms. May return a promise; the
         *  busy state holds until it resolves or rejects. The parent
         *  is expected to call `dialog.close()` on success. */
        onconfirm: () => void | Promise<void>;
    }

    let {
        dialog = $bindable(null),
        title,
        body,
        confirmLabel = 'Confirm',
        confirmBusyLabel = 'Working…',
        cancelLabel = 'Cancel',
        danger = false,
        onconfirm,
    }: Props = $props();

    let busy = $state<boolean>(false);

    function handleCancel() {
        dialog?.close();
    }

    async function handleConfirm() {
        busy = true;
        try {
            await onconfirm();
        } catch {
            // The parent surfaces the error in its own banner; we
            // just stop spinning. The dialog stays open so the user
            // can dismiss with Cancel/ESC after seeing the error.
        } finally {
            busy = false;
        }
    }
</script>

<dialog class="confirm-dialog" bind:this={dialog}>
    <h2>{title}</h2>
    {#if body}{@render body()}{/if}
    <div class="confirm-actions">
        <button type="button" onclick={handleCancel} disabled={busy}>
            {cancelLabel}
        </button>
        <button
            type="button"
            class:danger
            onclick={handleConfirm}
            disabled={busy}
        >
            {busy ? confirmBusyLabel : confirmLabel}
        </button>
    </div>
</dialog>

<style>
    /* Card-shaped confirmation. Same border + radius vocabulary as
       the surface elsewhere so the dialog reads as part of the app
       rather than the OS chrome the native `confirm` would surface. */
    .confirm-dialog {
        border: 1px solid var(--border);
        background: var(--bg-surface);
        color: var(--text-primary);
        border-radius: 10px;
        padding: 1.5rem 1.75rem;
        max-width: 28rem;
        width: calc(100vw - 2rem);
        font: inherit;
        box-shadow: 0 24px 48px rgba(0, 0, 0, 0.5);
    }

    .confirm-dialog::backdrop {
        background: rgba(0, 0, 0, 0.55);
    }

    .confirm-dialog h2 {
        font-size: 1.05rem;
        margin: 0 0 1rem;
        color: var(--text-label);
    }

    .confirm-actions {
        display: flex;
        justify-content: flex-end;
        gap: 0.6rem;
    }

    /* Destructive action gets red chrome on hover so the click
       still feels distinct from the safe Cancel. */
    .confirm-actions .danger {
        border-color: var(--error-border, #ef4444);
        color: var(--error-text, #ef4444);
    }

    .confirm-actions .danger:hover:not(:disabled) {
        background: var(--error-border, #ef4444);
        color: white;
    }
</style>
