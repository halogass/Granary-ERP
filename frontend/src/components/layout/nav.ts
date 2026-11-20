import {
  LayoutDashboard, Bot, Users, Settings, ClipboardCheck, MessageSquare, TrendingUp, DollarSign,
  FolderKanban, UserCheck, Megaphone, Cpu, Handshake, Truck, Headphones, Wrench, Car, PenLine,
  BookOpen, FlaskConical, MessageCircle, Mail,
  type LucideIcon,
} from "lucide-react";

/** Live counts the shell fetches; a link names the one it shows. */
export type BadgeKey = "approvals" | "nary" | "crm_email";

export interface NavLink {
  label: string;
  href: string;
  icon?: LucideIcon;
  /** Shown only to holders of this permission (the API checks it anyway). "a|b" means either one. */
  perm?: string;
  badge?: BadgeKey;
  /** Behind "Show more" inside its group, still in the palette. */
  extra?: boolean;
  /** A customer channel (CRM WhatsApp and email inbox): shown with its icon inside its group, as before. */
  channel?: boolean;
  /** Extra words the command palette matches. */
  keywords?: string[];
}

export interface NavGroup {
  label: string;
  icon: LucideIcon;
  links: NavLink[];
  perm?: string;
}

export interface NavSection {
  id: "top" | "modules" | "bottom";
  label?: string;
  /** Items are links, or groups that open into links (Inbox, the modules under More). */
  items: (NavLink | NavGroup)[];
  /** Folded until opened (More). */
  collapsible?: boolean;
}

export const NARY_VIEW = "nary:view";

/** The CRM email inbox: readable with either CRM view permission (the API checks the same). */
export const CRM_EMAIL_VIEW = "crm:view_all|crm:view_own";

/** The knowledge base: kb:view, or an editing permission (which includes reading), as the API checks. */
export const KB_VIEW = "kb:view|kb:create|kb:publish|kb:manage_sops";

/** True when `holds` grants the link's permission; "a|b" is satisfied by either code. */
export function permAllowed(perm: string | undefined, holds: (code: string) => boolean): boolean {
  if (!perm) return true;
  return perm.split("|").some((code) => holds(code.trim()));
}

export function isGroup(item: NavLink | NavGroup): item is NavGroup {
  return (item as NavGroup).links !== undefined;
}

// The menu tree is the same as before the redesign (the Owner asked for it exactly so): the same
// top items, the same 17 module groups in the same order with the same labels, Settings at the bottom.
export const NAV: NavSection[] = [
  {
    id: "top",
    items: [
      { label: "Dashboard", href: "/dashboard", icon: LayoutDashboard, keywords: ["today", "home"] },
      { label: "Approvals", href: "/approvals", icon: ClipboardCheck, badge: "approvals", keywords: ["sop", "requests"] },
      { label: "Nary", href: "/nary", icon: Bot, badge: "nary", keywords: ["assistant", "agent", "cards"] },
    ],
  },
  {
    id: "modules",
    items: [
      {
        label: "Messaging", icon: MessageSquare,
        links: [
          { label: "Direct Messages", href: "/messaging", keywords: ["messaging", "dm", "chat"] },
          { label: "Video Calls", href: "/calls", keywords: ["meet", "call"] },
          { label: "Email", href: "/email", keywords: ["my email", "mail"] },
        ],
      },
      {
        label: "CRM", icon: Users,
        links: [
          { label: "WhatsApp", href: "/crm/whatsapp", icon: MessageCircle, channel: true, keywords: ["chat", "wa", "inbox"] },
          { label: "Email Inbox", href: "/crm/email", icon: Mail, channel: true, badge: "crm_email", perm: CRM_EMAIL_VIEW, keywords: ["crm email", "inbox"] },
          { label: "Organizations", href: "/crm/organizations", keywords: ["companies", "accounts", "customers"] },
          { label: "Contacts", href: "/crm/contacts", keywords: ["people"] },
          { label: "Leads", href: "/crm/leads" },
          { label: "Deals", href: "/crm/deals", keywords: ["pipeline", "opportunities"] },
          { label: "Follow-ups", href: "/crm/followups", keywords: ["reminders"] },
          { label: "Activities", href: "/crm/activities" },
          { label: "Duplicates", href: "/crm/duplicates", keywords: ["merge"] },
        ],
      },
      {
        label: "Sales", icon: TrendingUp,
        links: [
          { label: "Quotations", href: "/sales/quotations", keywords: ["quotes"] },
          { label: "Proposals", href: "/sales/proposals" },
          { label: "Contracts", href: "/sales/contracts" },
          { label: "Product Catalog", href: "/sales/catalog", keywords: ["price list", "sku"] },
          // 5 Nov 2026: the new lines' spending gates and the monthly line report (Owner and system admins; the API checks)
        ],
      },
      {
        label: "Finance", icon: DollarSign,
        links: [
          { label: "Invoices", href: "/finance/invoices" },
          { label: "Payments", href: "/finance/payments" },
          { label: "Expenses", href: "/finance/expenses" },
          { label: "Payment Schedules", href: "/finance/schedules" },
          { label: "Chart of Accounts", href: "/finance/accounts" },
          { label: "Journal Entries", href: "/finance/journal" },
          { label: "Fiscal Periods", href: "/finance/periods" },
          { label: "Bank Reconciliation", href: "/finance/bank" },
          { label: "Taxes", href: "/finance/taxes" },
          { label: "Exchange Rates", href: "/finance/fx", keywords: ["fx", "currency"] },
          { label: "Reports", href: "/finance/reports" },
          { label: "Fixed Assets", href: "/finance/assets" },
          { label: "Asset Categories", href: "/finance/assets/categories" },
          { label: "Budget", href: "/finance/budget" },
        ],
      },
      {
        label: "Projects", icon: FolderKanban,
        links: [
          { label: "All Projects", href: "/projects" },
          { label: "My Tasks", href: "/projects/tasks" },
          { label: "Time Logs", href: "/projects/time-logs" },
        ],
      },
      {
        label: "HR", icon: UserCheck,
        links: [
          { label: "Org Chart", href: "/hr/org-chart" },
          { label: "Employees", href: "/hr/employees" },
          { label: "Attendance", href: "/hr/attendance" },
          { label: "Payroll", href: "/hr/payroll" },
          { label: "Leave", href: "/hr/leave" },
          { label: "Hiring", href: "/hr/hiring" },
          { label: "Job Openings", href: "/hr/job-openings" },
          { label: "Recruitment", href: "/hr/recruitment" },
          { label: "Offer Letters", href: "/hr/offer-letters" },
        ],
      },
      {
        label: "Marketing", icon: Megaphone,
        links: [
          { label: "Campaigns", href: "/marketing/campaigns" },
          { label: "Content Calendar", href: "/marketing/content" },
          { label: "Email Marketing", href: "/marketing/email" },
        ],
      },
      {
        label: "Manufacturing", icon: Cpu,
        links: [
          { label: "Products / BOMs", href: "/manufacturing/products" },
          { label: "Production Orders", href: "/manufacturing/production" },
          { label: "Unit Registry", href: "/manufacturing/units" },
          { label: "QC Records", href: "/manufacturing/qc" },
          { label: "Inventory", href: "/manufacturing/inventory" },
          { label: "Warranty / RMA", href: "/manufacturing/warranty" },
          { label: "Warehouses", href: "/manufacturing/warehouses" },
          { label: "Stock Movements", href: "/manufacturing/transfers" },
        ],
      },
      {
        label: "Partners", icon: Handshake,
        links: [
          { label: "Partner Accounts", href: "/partner-mgmt/accounts" },
          { label: "Deal Registrations", href: "/partner-mgmt/deals" },
          { label: "Commissions", href: "/partner-mgmt/commissions" },
          { label: "Resources", href: "/partner-mgmt/resources" },
        ],
      },
      {
        label: "Vendors", icon: Truck,
        links: [
          { label: "Vendor List", href: "/vendor-mgmt/vendors" },
          { label: "Contractor Requests", href: "/vendor-mgmt/contractor-requests" },
          { label: "Work Orders", href: "/vendor-mgmt/work-orders" },
          { label: "Timesheets", href: "/vendor-mgmt/timesheets", perm: "vendor_mgmt:view" },
          { label: "Purchase Orders", href: "/vendor-mgmt/purchase-orders" },
          { label: "Vendor Invoices", href: "/vendor-mgmt/invoices" },
          { label: "RFQs", href: "/vendor-mgmt/rfqs" },
          { label: "Goods Receipts", href: "/vendor-mgmt/receipts" },
        ],
      },
      {
        label: "Helpdesk", icon: Headphones,
        links: [
          { label: "Tickets", href: "/helpdesk/tickets" },
          { label: "Teams & SLA", href: "/helpdesk/teams" },
        ],
      },
      {
        label: "Maintenance", icon: Wrench,
        links: [
          { label: "Equipment", href: "/maintenance/equipment" },
          { label: "Requests", href: "/maintenance/requests" },
          { label: "Work Orders", href: "/maintenance/work-orders" },
          { label: "Schedules", href: "/maintenance/plans" },
        ],
      },
      {
        label: "Fleet", icon: Car,
        links: [
          { label: "Vehicles", href: "/fleet/vehicles" },
          { label: "Assignments", href: "/fleet/assignments" },
          { label: "Fuel Logs", href: "/fleet/fuel-logs" },
          { label: "Service Records", href: "/fleet/service" },
        ],
      },
      {
        label: "Signing", icon: PenLine,
        links: [
          { label: "Documents", href: "/sign/documents", keywords: ["e-sign", "signature"] },
        ],
      },
      {
        label: "Knowledge Base", icon: BookOpen,
        links: [
          { label: "Articles & SOPs", href: "/knowledge/articles", perm: KB_VIEW, keywords: ["sop", "docs"] },
          { label: "Documents", href: "/knowledge/documents", perm: "documents:view", keywords: ["brochure", "datasheet", "company profile", "price list", "library", "files"] },
          { label: "Categories", href: "/knowledge/categories", perm: `${KB_VIEW}|kb:manage_categories` },
        ],
      },
      {
        label: "R&D", icon: FlaskConical,
        links: [
          { label: "Projects", href: "/rnd/projects" },
          { label: "Backlog", href: "/rnd/backlog" },
          { label: "Product Lines", href: "/rnd/product-lines" },
        ],
      },
    ],
  },
  {
    id: "bottom",
    items: [
      {
        label: "Settings", icon: Settings,
        links: [
          { label: "General", href: "/settings", keywords: ["settings", "preferences", "config"] },
          { label: "Users", href: "/settings/users", keywords: ["roles", "permissions"] },
          { label: "Roles", href: "/settings/roles", keywords: ["permissions", "access"] },
          { label: "GitLab Integration", href: "/settings/gitlab" },
        ],
      },
    ],
  },
];

/** Every page the menu reaches, flat, with the place it sits ("Money", "More / Fleet"). */
export function allNavLinks(): (NavLink & { trail: string })[] {
  const out: (NavLink & { trail: string })[] = [];
  for (const s of NAV) {
    for (const item of s.items) {
      if (isGroup(item)) {
        const trail = s.label && s.label !== item.label ? `${s.label} / ${item.label}` : item.label;
        for (const l of item.links) out.push({ ...l, icon: l.icon ?? item.icon, perm: l.perm ?? item.perm, trail });
      } else {
        out.push({ ...item, trail: s.label ?? "" });
      }
    }
  }
  return out;
}

/** Pages not in the menu that the palette still offers. */
export const PALETTE_EXTRAS: NavLink[] = [
  { label: "Portal users", href: "/admin/portal-users", keywords: ["partner", "vendor", "login", "admin"] },
  { label: "Nary approvals", href: "/nary/approvals", keywords: ["cards", "pending"] },
  // Reached from Nary's own tabs and the partner pages, as before the redesign; here for Ctrl K.
  { label: "Nary chat", href: "/nary/chat", keywords: ["assistant"] },
  { label: "Nary meetings", href: "/nary/meetings", keywords: ["recording", "transcript", "debrief"] },
  { label: "Nary intake", href: "/nary/intake" },
  { label: "Nary history", href: "/nary/history", keywords: ["whatsapp import"] },
  { label: "Nary memory", href: "/nary/memory", keywords: ["skills", "facts", "style"] },
  { label: "Nary autonomy", href: "/nary/autonomy", keywords: ["levels"] },
  { label: "Nary report", href: "/nary/report", keywords: ["weekly"] },
  { label: "Nary runs", href: "/nary/runs" },
  // 3 Nov 2026: what waits while Nary is paused (Owner and system admins)
  { label: "Nary work queue", href: "/nary/queue", keywords: ["queue", "waiting", "todo", "mcp"] },
  { label: "Partner leads", href: "/partner-mgmt/leads" },
  { label: "Partner MDF", href: "/partner-mgmt/mdf", keywords: ["marketing development funds"] },
  { label: "Partner training", href: "/partner-mgmt/training" },
  { label: "My profile", href: "/profile", keywords: ["account", "password"] },
];

export function isActiveHref(pathname: string, href: string) {
  return pathname === href || pathname.startsWith(href + "/");
}

/** The most specific menu link for a path (so /finance/assets/categories does not also light up Fixed assets). */
export function activeHref(pathname: string): string | null {
  let best: string | null = null;
  for (const l of allNavLinks()) {
    if (isActiveHref(pathname, l.href) && (!best || l.href.length > best.length)) best = l.href;
  }
  return best;
}

// Icons the sidebar uses for its section headings when collapsed to a rail.
export const SECTION_ICONS: Record<NavSection["id"], LucideIcon> = {
  top: LayoutDashboard, modules: Users, bottom: Settings,
};
