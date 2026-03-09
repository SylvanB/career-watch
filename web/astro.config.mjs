import { defineConfig } from 'astro/config';
import tailwind from '@astrojs/tailwind';

export default defineConfig({
  integrations: [tailwind({ applyBaseStyles: false })],
  output: 'static',
  // In dev, proxy API calls to the Rust backend
  // In production, Caddy handles this
  vite: {
    server: {
      proxy: {
        '/api': 'http://localhost:8080',
        '/auth': 'http://localhost:8080',
        '/billing': 'http://localhost:8080',
        '/admin': 'http://localhost:8080',
      },
    },
  },
});
