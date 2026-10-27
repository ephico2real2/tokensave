import { render } from 'preact';
import { App, applyTheme } from './App';
import './app.css';
import { DEFAULT_THEME, readStoredTheme } from './state';

// Before the first render, so a stored Light choice doesn't flash the default Dark.
applyTheme(readStoredTheme() ?? DEFAULT_THEME);

const root = document.getElementById('app');
if (!root) throw new Error('tokensave dashboard: index.html has no #app element');
render(<App />, root);
