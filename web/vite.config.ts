import react from '@vitejs/plugin-react'
import {defineConfig} from 'vitest/config'

// The firmware serves every file from the root path and only knows a handful of
// assets, so emit a flat, single-chunk build: index.html + app.js + app.css.
export default defineConfig({
  plugins: [react()],
  build: {
    assetsDir: '',
    cssCodeSplit: false,
    chunkSizeWarningLimit: 2000,
    rollupOptions: {
      output: {
        entryFileNames: 'app.js',
        assetFileNames: 'app[extname]',
        codeSplitting: false,
      },
    },
  },
  server: {
    // `npm run dev` talks to the real device: DEVICE=<ESP32 IP> npm run dev
    proxy: {
      '/ws': {target: `ws://${process.env.DEVICE ?? 'localhost:8787'}`, ws: true},
    },
  },
  test: {environment: 'node'},
})
