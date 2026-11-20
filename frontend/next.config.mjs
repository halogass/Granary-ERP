import withSerwistInit from "@serwist/next";

const withSerwist = withSerwistInit({
  swSrc: "src/sw.ts",
  swDest: "public/sw.js",
  disable: process.env.NODE_ENV === "development",
  // Everything in public/ is precached except public/ort/ (the kiosk's onnxruntime-web, ~11 MB of wasm, copied
  // at build time): only kiosk tablets need it and they fetch it on first use.
  globPublicPatterns: ["*", "!(ort)/**/*"],
});

/** @type {import('next').NextConfig} */
const nextConfig = {
  output: "standalone",
  async headers() {
    return [
      {
        source: "/(.*)",
        headers: [
          { key: "X-Content-Type-Options", value: "nosniff" },
          { key: "X-Frame-Options", value: "SAMEORIGIN" },
          { key: "X-XSS-Protection", value: "1; mode=block" },
          { key: "Referrer-Policy", value: "strict-origin-when-cross-origin" },
          { key: "Permissions-Policy", value: "camera=(self), microphone=(self), geolocation=(self)" },
        ],
      },
    ];
  },
  // No remote images for the image optimiser: nothing uses next/image with a remote URL, and allowing
  // http://localhost or http://minio would let /_next/image fetch internal addresses for anyone.
  images: {
    remotePatterns: [],
  },
  async rewrites() {
    // INTERNAL_API_URL is the docker-internal address (http://backend:8000).
    // API_URL may be the public domain — never use it for server-side rewrites or
    // it creates an http→https redirect loop that leaks http:// URLs to the browser.
    const backendUrl = process.env.INTERNAL_API_URL || "http://backend:8000";
    return [
      {
        source: "/api/v1/:path*",
        destination: `${backendUrl}/api/v1/:path*`,
      },
      // MCP sign-in discovery lives at the site root (backend/app/routers/mcp_oauth.py). nginx may send these to
      // the backend itself (infra/nginx/granary.conf); when it doesn't, AI clients still find them.
      ...["oauth-protected-resource", "oauth-authorization-server"].map((doc) => ({
        source: `/.well-known/${doc}/:rest*`,
        destination: `${backendUrl}/.well-known/${doc}/:rest*`,
      })),
    ];
  },
};

export default withSerwist(nextConfig);
