import { createApp } from 'vue';
import { createPinia } from 'pinia';
import App from './App.vue';

// Register all @vscode-elements web components at once. Importing the package
// entry auto-defines every custom element (no per-component define call).
// Note: there is no `vscode-dropdown`; the select component is
// `vscode-single-select` / `vscode-multi-select`, both included here.
import '@vscode-elements/elements/dist/main.js';
import './style.css';

const app = createApp(App);
app.use(createPinia());
app.mount('#app');
