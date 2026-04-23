// Theme state lives as a `data-theme` attribute on <html>, stamped
// by the inline bootstrap script in app.html and by the toggle in
// the layout. Anything else that needs to react to the theme — for
// example, redrawing ECharts with theme-aware colors — can observe
// that attribute through this module without importing anything
// layout-specific.

export type Theme = 'light' | 'dark';

/** Current theme resolved from `<html data-theme>`. */
export function currentTheme(): Theme {
    if (typeof document === 'undefined') return 'light';
    return document.documentElement.dataset.theme === 'dark' ? 'dark' : 'light';
}

/**
 * Invoke `callback` with the current theme immediately, then again
 * every time `<html data-theme>` changes. Returns a disposer that
 * disconnects the observer — call it from the component's
 * `onMount` cleanup.
 */
export function observeTheme(callback: (theme: Theme) => void): () => void {
    if (typeof document === 'undefined') return () => {};
    callback(currentTheme());
    const obs = new MutationObserver(() => callback(currentTheme()));
    obs.observe(document.documentElement, {
        attributes: true,
        attributeFilter: ['data-theme'],
    });
    return () => obs.disconnect();
}
