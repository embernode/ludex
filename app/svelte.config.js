import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
export default {
    preprocess: vitePreprocess(),
    kit: {
        // SPA mode — Tauri serves the built app from a single
        // webview, no server-side rendering.
        adapter: adapter({ fallback: 'index.html' }),
    },
};
