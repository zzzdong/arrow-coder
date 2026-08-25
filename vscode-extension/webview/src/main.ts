import { createApp } from 'vue';
import { createPinia } from 'pinia';
import App from './App.vue';

// Register all @vscode-elements web components. The package entry `main.js`
// only re-exports the component classes; the bundled build auto-registers
// every custom element via `customElements.define`, which `main.js` does not.
// Without this, <vscode-textfield>/<vscode-tabs>/... are never defined and
// their inner <input> (etc.) never renders -> inputs can't be typed into.
import '@vscode-elements/elements/dist/bundled.js';
// Codicon icon font (the `.ac-codicon` glyphs used across the UI rely on the
// `codicon` @font-face defined here). Required for all toolbar/button icons.
import '@vscode/codicons/dist/codicon.css';
import './style.css';

const app = createApp(App);
app.use(createPinia());
app.mount('#app');
