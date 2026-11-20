import { ImageResponse } from "next/og";

export const runtime = "edge";

export function GET(_: Request, { params }: { params: { size: string } }) {
  const size = Math.min(Math.max(parseInt(params.size) || 192, 16), 1024);
  const fontSize = Math.round(size * 0.5);
  const radius = Math.round(size * 0.18);

  return new ImageResponse(
    (
      <div
        style={{
          width: size,
          height: size,
          backgroundColor: "#1f4e79",
          borderRadius: radius,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
        }}
      >
        <span
          style={{
            color: "#ffffff",
            fontSize,
            fontWeight: 800,
            fontFamily: "sans-serif",
            lineHeight: 1,
          }}
        >
          G
        </span>
      </div>
    ),
    { width: size, height: size }
  );
}
