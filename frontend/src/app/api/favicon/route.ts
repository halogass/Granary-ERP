import { NextResponse } from "next/server";

const API_URL = process.env.INTERNAL_API_URL || process.env.API_URL || "http://backend:8000";

/** The only types served: raster icons. Anything else (an SVG, HTML) could run script on this origin. */
const FAVICON_TYPES = ["image/png", "image/x-icon", "image/vnd.microsoft.icon", "image/jpeg"];

export async function GET() {
  try {
    const res = await fetch(`${API_URL}/api/v1/system/branding`, {
      next: { revalidate: 3600 }, // cache for 1 hour
    });
    if (!res.ok) throw new Error("branding fetch failed");

    const branding = await res.json();
    const faviconData: string = branding.favicon_data ?? "";

    const match = faviconData.match(/^data:([^;,]+);base64,(.+)$/);
    const mimeType = match?.[1].toLowerCase() ?? "";
    if (match && FAVICON_TYPES.includes(mimeType)) {
      const buffer = Buffer.from(match[2], "base64");
      return new NextResponse(buffer, {
        headers: {
          "Content-Type": mimeType,
          "Content-Security-Policy": "default-src 'none'; sandbox",
          "X-Content-Type-Options": "nosniff",
          "Cache-Control": "public, max-age=3600, stale-while-revalidate=86400",
        },
      });
    }
  } catch { /* fall through to default */ }

  // No custom favicon (or not a raster icon): 404 so the browser uses /favicon.ico
  return new NextResponse(null, { status: 404 });
}
