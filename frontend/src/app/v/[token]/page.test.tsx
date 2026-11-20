import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";

vi.mock("next-auth/react", () => ({ getSession: async () => null }));

import VerifyTokenPage from "./page";

const BASE = {
  state: "valid", valid: true, issuer: "Acme Ltd", entity_label: "Quotation",
  document_number: "Q-2026-11-00001-1", document_title: null, revision: 1,
  issued_at: "2026-11-03T03:00:00+00:00", expires_at: null, revoked_at: null, facts: [], approvals: [],
  signature: null, document_hash: "a".repeat(64), hash_algorithm: "SHA-256",
  can_view_document: false, allow_download: false,
};

function serve(body: object) {
  globalThis.fetch = vi.fn(async () => new Response(JSON.stringify(body), { status: 200 })) as typeof fetch;
}

beforeEach(() => vi.restoreAllMocks());

describe("public verification page: the outbound seal (3 Nov 2026)", () => {
  it("says the document is digitally sealed by the issuer, with its level and the sealed copy's hash", async () => {
    serve({ ...BASE, digitally_sealed: true, seal_level: "pades-b-b", sealed_hash: "b".repeat(64) });
    render(<VerifyTokenPage params={{ token: "t1" }} />);
    expect(await screen.findByText(/Digitally sealed by Acme Ltd \(PAdES-B-B\)/)).toBeInTheDocument();
    expect(screen.getByText(/of the digitally sealed copy/)).toBeInTheDocument();
    expect(screen.getByText("b".repeat(64))).toBeInTheDocument();
  });

  it("shows no seal for a document issued without one", async () => {
    serve({ ...BASE, digitally_sealed: false, seal_level: null, sealed_hash: null });
    render(<VerifyTokenPage params={{ token: "t2" }} />);
    expect(await screen.findByText("Authentic document")).toBeInTheDocument();
    expect(screen.queryByText(/Digitally sealed by/)).toBeNull();
    expect(screen.queryByText(/of the digitally sealed copy/)).toBeNull();
  });
});
