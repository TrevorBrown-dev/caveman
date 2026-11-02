import { defineConfig, type Plugin } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { readFileSync, existsSync, statSync } from 'node:fs';
import { resolve, normalize } from 'node:path';

/**
 * `pnpm dev:mock`: the app in a plain browser with a stand-in back end (mock/library.json, written by the back end's
 * `dump_mock_fixture` test), so the UI can be reviewed without the Tauri window. Image files come through
 * `/__mock/file?path=...`, limited to the folders the fixture names. `CAVEMAN_MOCK_BOARD=<a .glb>` gives the Studio's
 * preview the game's board (`cargo test -p caveman-core --test board_model -- --ignored` writes some to target/).
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
        // the Studio's board model: a .glb named by CAVEMAN_MOCK_BOARD (the board_model tests write some to target/)
        const board = process.env.CAVEMAN_MOCK_BOARD ? normalize(resolve(process.env.CAVEMAN_MOCK_BOARD)) : null;
        if (url.pathname === '/__mock/board') {
          if (!board || !existsSync(board)) {
            res.statusCode = 404;
            res.end();
            return;
          }
          res.setHeader('Content-Type', 'application/json');
          res.end(JSON.stringify({ path: board }));
          return;
        }
        if (url.pathname === '/__mock/file') {
          const fixture = existsSync(fixturePath) ? JSON.parse(readFileSync(fixturePath, 'utf8')) : { roots: [] };
          const path = normalize(url.searchParams.get('path') ?? '');
          const allowed = (fixture.roots as string[]).some((root) => path.toLowerCase().startsWith(normalize(root).toLowerCase())) || (board !== null && path.toLowerCase() === board.toLowerCase());
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
  // fs.allow: the changelog (../CHANGELOG.md) is imported from outside app/ (lib/changelog.ts)
  server: { port: 5173, strictPort: true, fs: { allow: ['..'] } },
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  build: {
    target: 'es2022',
    chunkSizeWarningLimit: 1500,
  },
}));
