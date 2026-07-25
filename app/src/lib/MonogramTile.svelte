<script lang="ts">
    interface Props {
        /** Product name; the tile shows its first character. */
        name: string;
        /** Edge length in pixels. */
        size?: number;
    }
    let { name, size = 34 }: Props = $props();

    // Segment by grapheme where the engine offers it, so a name
    // starting with a combining mark, an emoji ZWJ sequence, or a
    // flag yields the whole cluster. Spreading a string only splits
    // by code point, which keeps surrogate pairs intact but would
    // drop the accent off a decomposed "é" — hence the segmenter
    // first, with the spread as the fallback.
    const initial = $derived.by(() => {
        const trimmed = name.trim();
        if (!trimmed) return '?';
        if (typeof Intl !== 'undefined' && 'Segmenter' in Intl) {
            const segmenter = new Intl.Segmenter(undefined, {
                granularity: 'grapheme',
            });
            const first = segmenter.segment(trimmed)[Symbol.iterator]().next();
            if (!first.done) return first.value.segment.toUpperCase();
        }
        return [...trimmed][0].toUpperCase();
    });
</script>

<!-- The design chose a monogram over cover art deliberately: rows with
     no art would otherwise change the layout, which is why this is a
     table rather than a poster grid. It also means no art pipeline. -->
<span
    class="tile"
    aria-hidden="true"
    style="width:{size}px;height:{size}px;border-radius:{size >= 48
        ? 7
        : 5}px;font-size:{Math.round(size * 0.31)}px"
>
    {initial}
</span>

<style>
    .tile {
        display: flex;
        align-items: center;
        justify-content: center;
        font-family: 'JetBrains Mono', ui-monospace, Menlo, monospace;
        font-weight: 500;
        background: var(--tile);
        color: var(--fg3);
        flex: none;
    }
</style>
