import { defineConfig } from 'vite';
import vue from '@vitejs/plugin-vue';
import { resolve, dirname } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));

// Build the Vue 3 webview SPA into out/webview, where ChatPanel.ts loads it via
// asWebviewUri. The bundle is an ES module (type="module") referenced from the
// webview HTML with a nonce, so the CSP allows 'nonce-{N}'. No inline scripts.
export default defineConfig({
  root: resolve(__dirname, 'webview'),
  plugins: [
    vue({
      template: {
        compilerOptions: {
          // Treat @vscode-elements web components as custom elements so the
          // Vue compiler doesn't warn about unknown tags.
          isCustomElement: (tag) => tag.startsWith('vscode-'),
        },
      },
    }),
  ],
  build: {
    outDir: resolve(__dirname, 'out/webview'),
    emptyOutDir: true,
    target: 'es2020',
    rollupOptions: {
      input: resolve(__dirname, 'webview/index.html'),
      output: {
        // Emit a stable bundle name so ChatPanel.ts can reference it without
        // guessing the Vite content hash. Stable names are fine here — the
        // webview is always reloaded on extension (re)load.
        entryFileNames: 'assets/index.js',
        chunkFileNames: 'assets/[name].js',
        assetFileNames: 'assets/[name].[ext]',
      },
    },
  },
});
