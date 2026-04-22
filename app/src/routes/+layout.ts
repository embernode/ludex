// SPA mode — the app is served by Tauri's webview, not a node
// runtime, so there is no server-side rendering to do and no
// prerender step is meaningful.
export const prerender = false;
export const ssr = false;
