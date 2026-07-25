<script lang="ts">
    interface Props {
        checked: boolean;
        label: string;
        onchange: (next: boolean) => void;
    }
    let { checked, label, onchange }: Props = $props();
</script>

<button
    type="button"
    class="switch"
    role="switch"
    aria-checked={checked}
    aria-label={label}
    onclick={() => onchange(!checked)}
>
    <i></i>
</button>

<style>
    .switch {
        width: 34px;
        height: 19px;
        border-radius: 99px;
        background: var(--track);
        position: relative;
        cursor: pointer;
        border: 0;
        padding: 0;
        flex: none;
    }

    .switch[aria-checked='true'] {
        background: var(--ac);
    }

    .switch:focus-visible {
        outline: 2px solid var(--ac);
        outline-offset: 2px;
    }

    .switch i {
        position: absolute;
        top: 2px;
        left: 2px;
        width: 15px;
        height: 15px;
        border-radius: 99px;
        background: #fff;
        transition: left 0.12s;
    }

    .switch[aria-checked='true'] i {
        left: 17px;
    }

    @media (prefers-reduced-motion: reduce) {
        .switch i {
            transition: none;
        }
    }
</style>
