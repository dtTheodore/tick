import path from 'node:path';
import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

export default defineConfig(({ command }) => {
  const isDev = command === 'serve';
  const port = Number(process.env.TAP_UI_PORT);
  if (isDev && (!Number.isInteger(port) || port <= 0)) {
    throw new Error('TAP_UI_PORT not set; run ./scripts/init-worktree-dev.sh first');
  }

  return {
    plugins: [react(), tailwindcss()],
    resolve: {
      alias: {
        '@': path.resolve(__dirname, './src'),
      },
    },
    ...(isDev && {
      server: {
        port,
        strictPort: true,
        cors: true,
      },
    }),
  };
});
