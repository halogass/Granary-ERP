import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import InvoiceClientCard from "./InvoiceClientCard";

describe("Invoice client card", () => {
  it("shows the organization and contact the invoice API serves", () => {
    render(<InvoiceClientCard invoice={{
      customer_org_id: "o1", customer_contact_id: "c1",
      customer_org: { id: "o1", name: "Aozora Accounting Partners", email: "billing@aozora.example.com" },
      customer_contact: { id: "c1", full_name: "Sora Fujii", email: "sora@aozora.example.com" },
      customer_name: "Aozora Accounting Partners", customer_email: "billing@aozora.example.com",
    }} />);
    expect(screen.getByRole("link", { name: "Aozora Accounting Partners" })).toHaveAttribute("href", "/crm/organizations/o1");
    expect(screen.getByRole("link", { name: "Sora Fujii" })).toHaveAttribute("href", "/crm/contacts/c1");
    expect(screen.getByText("sora@aozora.example.com")).toBeInTheDocument();
    expect(screen.queryByText("No client set.")).not.toBeInTheDocument();
  });

  it("falls back to the bill-to name and email on the invoice", () => {
    render(<InvoiceClientCard invoice={{ customer_org_id: "o1", customer_org: null, customer_contact: null,
      customer_name: "Hoshino Design Office", customer_email: "office@hoshino.example.com" }} />);
    expect(screen.getByText("Hoshino Design Office")).toBeInTheDocument();
    expect(screen.getByText("office@hoshino.example.com")).toBeInTheDocument();
    expect(screen.queryByText("No client set.")).not.toBeInTheDocument();
  });

  it("says so when the invoice has no client", () => {
    render(<InvoiceClientCard invoice={{ customer_org: null, customer_contact: null, customer_name: null }} />);
    expect(screen.getByText("No client set.")).toBeInTheDocument();
  });
});
