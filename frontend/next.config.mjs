import {
  PHASE_DEVELOPMENT_SERVER,
  PHASE_PRODUCTION_BUILD,
  PHASE_PRODUCTION_SERVER,
} from 'next/constants.js';

/** @type {import('next').NextConfig} */
export default function nextConfig(phase) {
  const isProductionBuild = phase === PHASE_PRODUCTION_BUILD;
  const usesProductionArtifacts =
    phase === PHASE_PRODUCTION_BUILD || phase === PHASE_PRODUCTION_SERVER;

  return {
    // Static export in production builds, but keep the dev server dynamic so
    // route changes and rewrites behave like a normal app server.
    output: isProductionBuild ? 'export' : undefined,
    // Keep production builds from clobbering the live dev server's `.next`
    // artifacts. Running `next build` while `next dev` is active otherwise
    // leaves the dev server pointing at stale asset aliases that 404.
    distDir: usesProductionArtifacts ? '.next-build' : '.next',

    // Disable image optimization for static export
    images: {
      unoptimized: true,
    },

    // Trailing slashes for static file serving
    trailingSlash: false,

    ...(phase === PHASE_DEVELOPMENT_SERVER
      ? {
          // Dev-only rewrites keep deep links loadable without defining
          // export-hostile dynamic app routes. The browser URL stays intact and
          // the client router still parses the pathname after hydration.
          async rewrites() {
            return [
              {
                source: '/c/:contextId/t/:turnId',
                destination: '/',
              },
              {
                source: '/c/:contextId',
                destination: '/',
              },
              {
                source: '/v1/:path*',
                destination: 'http://127.0.0.1:9010/v1/:path*',
              },
              {
                source: '/healthz',
                destination: 'http://127.0.0.1:9010/healthz',
              },
            ];
          },
        }
      : {}),
  };
}
