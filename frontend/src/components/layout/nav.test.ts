import { describe, it, expect } from "vitest";
import { readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { NAV, PALETTE_EXTRAS, activeHref, allNavLinks, isGroup } from "./nav";
import { recordListPath } from "@/lib/recent";

// Every link of the menu before the redesign (Sidebar.tsx, Oct 2026). None may disappear.
const OLD_LINKS = [
  "/dashboard", "/approvals", "/nary", "/messaging", "/calls", "/email",
  "/crm/whatsapp", "/crm/email", "/crm/organizations", "/crm/contacts", "/crm/leads", "/crm/deals",
  "/crm/followups", "/crm/activities", "/crm/duplicates",
  "/sales/quotations", "/sales/proposals", "/sales/contracts", "/sales/catalog",
  "/finance/invoices", "/finance/payments", "/finance/expenses", "/finance/schedules", "/finance/accounts",
  "/finance/journal", "/finance/periods", "/finance/bank", "/finance/taxes", "/finance/fx", "/finance/reports",
  "/finance/assets", "/finance/assets/categories", "/finance/budget",
  "/projects", "/projects/tasks", "/projects/time-logs",
  "/hr/org-chart", "/hr/employees", "/hr/attendance", "/hr/payroll", "/hr/leave", "/hr/hiring",
  "/hr/job-openings", "/hr/recruitment", "/hr/offer-letters",
  "/marketing/campaigns", "/marketing/content", "/marketing/email",
  "/manufacturing/products", "/manufacturing/production", "/manufacturing/units", "/manufacturing/qc",
  "/manufacturing/inventory", "/manufacturing/warranty", "/manufacturing/warehouses", "/manufacturing/transfers",
  "/partner-mgmt/accounts", "/partner-mgmt/deals", "/partner-mgmt/commissions", "/partner-mgmt/resources",
  "/vendor-mgmt/vendors", "/vendor-mgmt/contractor-requests", "/vendor-mgmt/work-orders",
  "/vendor-mgmt/purchase-orders", "/vendor-mgmt/invoices", "/vendor-mgmt/rfqs", "/vendor-mgmt/receipts",
  "/helpdesk/tickets", "/helpdesk/teams",
  "/maintenance/equipment", "/maintenance/requests", "/maintenance/work-orders", "/maintenance/plans",
  "/fleet/vehicles", "/fleet/assignments", "/fleet/fuel-logs", "/fleet/service",
  "/sign/documents", "/knowledge/articles", "/knowledge/categories",
  "/rnd/projects", "/rnd/backlog", "/rnd/product-lines",
  "/settings", "/settings/users", "/settings/gitlab", "/profile",
];

const ERP = join(__dirname, "../../app/(erp)");

function pageRoutes(dir: string, base = ""): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const full = join(dir, name);
    if (!statSync(full).isDirectory()) {
      if (name === "page.tsx") out.push(base || "/");
      continue;
    }
    out.push(...pageRoutes(full, `${base}/${name}`));
  }
  return out;
}

describe("navigation", () => {
  const hrefs = new Set([...allNavLinks().map((l) => l.href), ...PALETTE_EXTRAS.map((l) => l.href)]);

  it("keeps every link the old sidebar had", () => {
    expect(OLD_LINKS.filter((h) => !hrefs.has(h))).toEqual([]);
  });

  it("only links to pages that exist", () => {
    const routes = new Set(pageRoutes(ERP));
    expect(Array.from(hrefs).filter((h) => !routes.has(h))).toEqual([]);
  });

  it("reaches every list page of the ERP (record, form and redirect pages aside)", () => {
    const skip = /\[|\/new$|^\/style-guide$|^\/assistant$|^\/crm\/inbox$|^\/crm\/search$|^\/marketing\/email\//;
    const unreachable = pageRoutes(ERP).filter((r) => !skip.test(r) && !hrefs.has(r));
    expect(unreachable).toEqual([]);
  });

  it("keeps the menu tree of before the redesign: top items, 16 module groups in order, Settings", () => {
    const all = allNavLinks().map((l) => l.href);
    expect(all.length).toBe(new Set(all).size);
    expect(NAV.map((s) => s.id)).toEqual(["top", "modules", "bottom"]);
    expect(NAV[0].items.map((i) => i.label)).toEqual(["Dashboard", "Approvals", "Nary"]);
    expect(NAV[1].items.map((i) => i.label)).toEqual([
      "Messaging", "CRM", "Sales", "Finance", "Projects", "HR", "Marketing", "Manufacturing", "Partners",
      "Vendors", "Helpdesk", "Maintenance", "Fleet", "Signing", "Knowledge Base", "R&D",
    ]);
    expect(NAV[1].items.every(isGroup)).toBe(true);
    const kb = NAV[1].items.find((i) => i.label === "Knowledge Base");
    expect(kb && isGroup(kb) && kb.links.map((l) => l.label)).toEqual(["Articles & SOPs", "Documents", "Categories"]);
    const settings = NAV[2].items[0];
    expect(isGroup(settings) && settings.links.map((l) => l.label)).toEqual(["General", "Users", "Roles", "GitLab Integration"]);
  });

  it("lights the most specific link and finds record pages", () => {
    expect(activeHref("/finance/assets/categories")).toBe("/finance/assets/categories");
    expect(activeHref("/finance/assets/123")).toBe("/finance/assets");
    expect(activeHref("/crm/deals/7f3e2a10-1111-2222-3333-444455556666")).toBe("/crm/deals");
    expect(recordListPath("/crm/deals/7f3e2a10-1111-2222-3333-444455556666")).toBe("/crm/deals");
    expect(recordListPath("/sales/quotations/12/edit")).toBe("/sales/quotations");
    expect(recordListPath("/crm/deals")).toBeNull();
    expect(recordListPath("/sales/quotations/new")).toBeNull();
  });
});
