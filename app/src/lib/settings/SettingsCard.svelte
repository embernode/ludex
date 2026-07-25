<script lang="ts">
    import type { Snippet } from 'svelte';

    interface Props {
        /** Card heading. Omit for a card that is a single bare row. */
        title?: string;
        /** One-line explanation under the heading. */
        subtitle?: string;
        children: Snippet;
    }
    let { title, subtitle, children }: Props = $props();
</script>

<section class="card">
    {#if title}
        <div class="cardhead">
            <div class="cardtitle">{title}</div>
            {#if subtitle}
                <div class="cardsub">{subtitle}</div>
            {/if}
        </div>
    {/if}
    {@render children()}
</section>

<style>
    .card {
        background: var(--surface);
        border: 1px solid var(--line);
        border-radius: 9px;
        margin-bottom: 14px;
    }

    .cardhead {
        padding: 14px 16px 12px;
        border-bottom: 1px solid var(--line);
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
</style>
