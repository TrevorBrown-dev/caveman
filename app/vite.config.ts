import { defineConfig, type Plugin } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { readFileSync, existsSync, statSync } from 'node:fs';
import { resolve, normalize } from 'node:path';

/**
 * `pnpm dev:mock`: the app in a plain browser with a stand-in back end (mock/library.json, written by the back end's
 * `dump_mock_fixture` test), so the UI can be reviewed without the Tauri window. Image files come through
 * `/__mock/file?path=...`, limited to the folders the fixture names.
 */
function mockBackend(): Plugin {
  const fixturePath = resolve(import.meta.dirname, 'mock', 'library.json');
  return {
    name: 'caveman-mock-backend',
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        if (!req.url?.startsWith('/__mock/')) {
          next();
          return;
        }
        const url = new URL(req.url, 'http://localhost');
        if (url.pathname === '/__mock/library') {
          res.setHeader('Content-Type', 'application/json');
          res.end(existsSync(fixturePath) ? readFileSync(fixturePath) : '{"error": "no mock/library.json"}');
          return;
        }
        if (url.pathname === '/__mock/file') {
          const fixture = existsSync(fixturePath) ? JSON.parse(readFileSync(fixturePath, 'utf8')) : { roots: [] };
          const path = normalize(url.searchParams.get('path') ?? '');
          const allowed = (fixture.roots as string[]).some((root) => path.toLowerCase().startsWith(normalize(root).toLowerCase()));
          if (!allowed || !existsSync(path) || !statSync(path).isFile()) {
            res.statusCode = 404;
            res.end();
            return;
          }
          res.setHeader('Content-Type', path.toLowerCase().endsWith('.png') ? 'image/png' : 'application/octet-stream');
          res.setHeader('Cache-Control', 'no-cache');
          res.end(readFileSync(path));
          return;
        }
        res.statusCode = 404;
        res.end();
      });
    },
  };
}

export default defineConfig(({ mode }) => ({
  plugins: [svelte(), ...(mode === 'mock' ? [mockBackend()] : [])],
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  build: {
    target: 'es2022',
    chunkSizeWarningLimit: 1500,
  },
}));
