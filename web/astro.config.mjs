import { defineConfig } from 'astro/config';
import tailwindcss from '@tailwindcss/vite';

// Shared proxy target + options — explicit configuration ensures the Cookie
// header is forwarded and the Host header matches what the Rust API expects.
const API = 'http://localhost:8080';
const proxyOpts = {
  target: API,
  changeOrigin: false,
  // Forward cookies without rewriting; required for session auth via Vite proxy.
  cookieDomainRewrite: { '*': '' },
  configure(proxy) {
    proxy.on('proxyReq', (proxyReq, req) => {
      // Explicitly propagate the Cookie header so the session cookie reaches
      // the Rust API even when the browser treats the request as same-site.
      if (req.headers.cookie) {
        proxyReq.setHeader('Cookie', req.headers.cookie);
      }
    });
  },
};

export default defineConfig({
  output: 'static',
  vite: {
    plugins: [tailwindcss()],
    server: {
      proxy: {
        '/api':     proxyOpts,
        '/auth':    proxyOpts,
        '/billing': proxyOpts,
        '/admin':   proxyOpts,
      },
    },
  },
});
