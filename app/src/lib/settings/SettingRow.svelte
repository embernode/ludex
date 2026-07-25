<script lang="ts">
    import type { Snippet } from 'svelte';

    interface Props {
        label: string;
        /** Explanatory line under the label. */
        help?: string;
        /**
         * Validation failure for this row. Rendered in place of the
         * help text so a rejected entry can't scroll out of sight —
         * with instant apply there is no dirty state left to signal
         * that something didn't take.
         */
        error?: string | null;
        /** Ties the message to its control via `aria-describedby`. */
        errorId?: string;
        /** The control on the right-hand side. */
        control: Snippet;
    }
    let { label, help, error = null, errorId, control }: Props = $props();
</script>

<div class="setrow">
    <div>
        <div class="setlabel">{label}</div>
        {#if error}
            <div class="seterror" id={errorId} role="alert">{error}</div>
        {:else if help}
            <div class="sethelp">{help}</div>
        {/if}
    </div>
    <div class="control">{@render control()}</div>
</div>

<style>
    .setrow {
        display: grid;
        grid-template-columns: 1fr auto;
        gap: 14px;
        align-items: center;
        padding: 13px 16px;
        border-top: 1px solid var(--hair);
    }

    /* Only bites in an untitled card, where the row really is the
       first element — in a titled card the head is the first sibling,
       so every row keeps its rule and the head's own border does the
       separating. Matches the mockup's structure and result. */
    .setrow:first-of-type {
        border-top: 0;
    }

    .setlabel {
        font-size: 12.5px;
        font-weight: 500;
        color: var(--fg2);
    }

    .sethelp {
        font-size: 11.5px;
        line-height: 1.45;
        margin-top: 3px;
        color: var(--fg3);
    }

    /* `--error-text`, not `--warn`: the design has no error state, and
       the warn tint is #e08b6a, which lands at 2.6:1 on the light
       surface — under the 4.5:1 floor for the one piece of text
       explaining why a value was refused. */
    .seterror {
        font-size: 11.5px;
        line-height: 1.45;
        margin-top: 3px;
        color: var(--error-text);
    }

    .control {
        display: flex;
        align-items: center;
        gap: 8px;
    }
</style>
