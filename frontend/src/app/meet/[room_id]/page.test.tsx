import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";

vi.mock("next/navigation", () => ({
  useParams: () => ({ room_id: "r1" }),
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  usePathname: () => "/meet/r1",
  useSearchParams: () => new URLSearchParams(),
}));
vi.mock("@/hooks/useBranding", () => ({
  useBranding: () => ({ logo_type: "text", logo_text: "Kitsune Trading", logo_subtitle: "ERP", logo_image_data: "" }),
}));
vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  guestCallsApi: {
    getRoomPublic: () => Promise.resolve({ name: "Weekly review", is_active: true }),
    enterWaiting: vi.fn(),
    pollStatus: vi.fn(),
  },
}));

import GuestMeetPage from "./page";

describe("guest meeting page header", () => {
  it("shows the company branding, not a fixed letter and product name", async () => {
    render(<GuestMeetPage />);
    expect(await screen.findByText("· Weekly review")).toBeInTheDocument();
    const header = document.querySelector("header")!;
    expect(header.textContent).toContain("Kitsune Trading");
    expect(header.textContent).not.toContain("Granary");
  });
});
