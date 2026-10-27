import preact from '@preact/preset-vite';
import { defineConfig } from 'vitest/config';

// The build writes dist/: index.html, tokens.css (copied from public/), app.js and app.css,
// with fixed names. dist/ is committed and embedded in the tokensave binary, so `cargo build`
// never needs Node; CI rebuilds it and fails when it differs from the committed files.
export default defineConfig({
  plugins: [preact()],
  base: './',
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    assetsDir: '',
    modulePreload: { polyfill: false },
    rolldownOptions: {
      output: {
        entryFileNames: 'app.js',
        chunkFileNames: 'app-[name].js',
        assetFileNames: 'app[extname]',
      },
    },
  },
  // The tokensave server sends this policy; previewing under it proves the page needs nothing else.
  preview: {
    headers: { 'Content-Security-Policy': "default-src 'self'" },
  },
  test: {
    environment: 'jsdom',
    include: ['tests/unit/**/*.test.{ts,tsx}'],
  },
});
