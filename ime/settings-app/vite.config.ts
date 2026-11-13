import { defineConfig } from 'vite';
// modulePreload.polyfill off: WebView2 supports modulepreload natively, and the bundle then contains only this
// project's own code (THIRD-PARTY-NOTICES.md: no npm package is bundled into the frontend).
export default defineConfig({ base: './', build: { outDir: 'dist', emptyOutDir: true, modulePreload: { polyfill: false } }, server: { port: 5173, strictPort: true } });
