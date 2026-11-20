/**
 * Shared document print/PDF utility.
 * Fetches the company letterhead from the API and renders
 * the letterhead as an <img> at the top of every printed document.
 */

import { systemApi } from "./api";
import { sanitizeHtml } from "./sanitize";
import { formatCurrency } from "./currency";
import { lineTermText, orderLineTree } from "./catalogPricing";

interface CompanySettings {
  company_name: string;
  company_trade_name: string;
  company_email: string;
  company_phone: string;
  company_address: string;
  company_rd_office: string;
  company_website: string;
  company_tax_id: string;
  company_registration_no: string;
  company_signatory: string;
  company_signatory_title: string;
  [key: string]: string;
}

const COMPANY_FALLBACK: CompanySettings = {
  company_name: "Your Company Ltd",
  company_trade_name: "Your Company",
  company_email: "hello@example.com",
  company_phone: "",
  company_address: "",
  company_rd_office: "",
  company_website: "https://example.com",
  company_tax_id: "",
  company_registration_no: "",
  company_signatory: "",
  company_signatory_title: "",
};

let _cachedCompany: CompanySettings | undefined = undefined;

async function fetchCompany(): Promise<CompanySettings> {
  if (_cachedCompany !== undefined) return _cachedCompany;
  try {
    const res = await systemApi.getCompanySettings();
    _cachedCompany = { ...COMPANY_FALLBACK, ...res.data };
  } catch {
    _cachedCompany = { ...COMPANY_FALLBACK };
  }
  return _cachedCompany!;
}

export function invalidateCompanyCache(): void {
  _cachedCompany = undefined;
}

const BASE_STYLES = `
*{box-sizing:border-box;margin:0;padding:0}
body{font-family:Georgia,'Times New Roman',serif;max-width:800px;margin:0 auto;padding:0 32px 40px;color:#1a1a1a;line-height:1.7;font-size:14px}
img.letterhead{width:100%;display:block;margin-bottom:0}
.letterhead-divider{border-top:2px solid #1f4e79;margin-bottom:22px}
.doc-title{font-family:Arial,sans-serif;font-size:26px;font-weight:900;color:#1f4e79;letter-spacing:0.5px;margin-bottom:4px}
.doc-number{font-family:'Courier New',monospace;font-size:13px;color:#555;letter-spacing:0.5px}
.doc-header{display:flex;justify-content:space-between;align-items:flex-start;margin-bottom:20px;padding-bottom:14px;border-bottom:1px solid #e5e7eb}
.doc-meta{font-family:Arial,sans-serif;font-size:12px;color:#666}
.doc-meta strong{color:#222;display:inline-block;width:80px}
.customer-block{background:#f9fafb;border:1px solid #e5e7eb;border-radius:8px;padding:14px 18px;margin-bottom:20px;font-family:Arial,sans-serif;font-size:13px}
.customer-block .label{font-size:10px;text-transform:uppercase;letter-spacing:1px;color:#9ca3af;font-weight:700;margin-bottom:6px}
.customer-block .name{font-size:16px;font-weight:800;color:#111;margin-bottom:2px}
.customer-block .detail{color:#555;font-size:12px}
table.items{width:100%;border-collapse:collapse;margin-bottom:16px;font-size:13px}
table.items thead th{background:#f3f4f6;padding:8px 12px;text-align:left;font-family:Arial,sans-serif;font-weight:700;font-size:11px;text-transform:uppercase;letter-spacing:0.5px;color:#374151;border-bottom:2px solid #e5e7eb}
table.items thead th.right{text-align:right}
table.items tbody td{padding:10px 12px;border-bottom:1px solid #f3f4f6;vertical-align:top}
table.items tbody td.right{text-align:right;font-family:'Courier New',monospace}
table.items tbody tr:last-child td{border-bottom:none}
.sku{font-size:10px;color:#9ca3af;font-family:'Courier New',monospace;margin-bottom:2px}
.totals{margin-left:auto;width:260px;font-size:13px;font-family:Arial,sans-serif}
.totals .row{display:flex;justify-content:space-between;padding:5px 0;color:#555}
.totals .row.total{font-weight:900;font-size:16px;color:#111;border-top:2px solid #111;margin-top:6px;padding-top:8px}
.totals .row.discount{color:#f97316}
.totals .row.tax{color:#3b82f6}
.section-title{font-family:Arial,sans-serif;font-size:10px;font-weight:700;text-transform:uppercase;letter-spacing:1px;color:#9ca3af;margin-bottom:8px;margin-top:24px}
.notes{font-size:13px;color:#555;line-height:1.7}
.notes ul{list-style:disc;padding-left:20px;margin:4px 0}
.notes ol{list-style:decimal;padding-left:20px;margin:4px 0}
.notes a{color:#1f4e79;text-decoration:underline}
.notes p{margin:4px 0}
.status-badge{display:inline-block;padding:3px 10px;border-radius:20px;font-family:Arial,sans-serif;font-size:11px;font-weight:700;text-transform:uppercase;letter-spacing:0.5px}
.status-paid{background:#dcfce7;color:#16a34a}
.status-sent{background:#dbeafe;color:#1d4ed8}
.status-overdue{background:#fee2e2;color:#dc2626}
.status-draft{background:#f1f5f9;color:#475569}
.payment-history{margin-top:20px}
.payment-row{display:flex;justify-content:space-between;padding:8px 12px;border-radius:6px;background:#f9fafb;margin-bottom:4px;font-size:12px;font-family:Arial,sans-serif}
.signature-block{margin-top:40px;display:flex;justify-content:flex-end}
.signature-inner{text-align:center;font-family:Arial,sans-serif;font-size:12px;color:#555;width:200px}
.signature-line{border-top:1px solid #9ca3af;margin-bottom:6px;margin-top:48px}
.signature-name{font-weight:700;color:#111;font-size:13px}
.signature-title{font-size:11px;color:#9ca3af}
.footer-note{margin-top:32px;padding-top:12px;border-top:1px solid #e5e7eb;font-size:11px;color:#9ca3af;font-family:Arial,sans-serif;text-align:center}
@media print{
  @page{margin:15mm 12mm}
  body{padding:0;max-width:100%}
  .no-print{display:none!important}
}
`;

export interface PrintDocumentOptions {
  title: string;
  contentHTML: string;
  extraStyles?: string;
}

let _cachedLetterhead: string | null | undefined = undefined;

async function fetchLetterhead(): Promise<string | null> {
  if (_cachedLetterhead !== undefined) return _cachedLetterhead;
  try {
    const res = await systemApi.getLetterhead();
    _cachedLetterhead = res.data.data_url ?? null;
  } catch {
    _cachedLetterhead = null;
  }
  return _cachedLetterhead!;
}

/** Invalidate the in-memory letterhead cache (call after upload/delete). */
export function invalidateLetterheadCache(): void {
  _cachedLetterhead = undefined;
}

export async function printDocument(options: PrintDocumentOptions): Promise<void> {
  const [letterhead] = await Promise.all([fetchLetterhead(), fetchCompany()]);

  const headerHtml = letterhead
    ? `<img class="letterhead" src="${letterhead}" alt="Company letterhead" /><div class="letterhead-divider"></div>`
    : "";

  const html = `<!DOCTYPE html><html lang="en"><head>
<meta charset="UTF-8">
<title>${escHtml(options.title)}</title>
<style>
${BASE_STYLES}
${options.extraStyles ?? ""}
</style>
</head><body>
${headerHtml}
${options.contentHTML}
<script>window.onload=()=>{setTimeout(()=>{window.print()},400)}<\/script>
</body></html>`;

  const win = window.open("", "_blank", "width=900,height=800");
  if (!win) {
    alert("Pop-up blocked. Allow pop-ups for this site and try again.");
    return;
  }
  win.document.write(html);
  win.document.close();
}

function escHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

function fmtDate(dateStr: string | undefined | null): string {
  if (!dateStr) return "-";
  return new Date(dateStr).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" });
}

// Delegates to the shared formatter so a printed document uses the same symbol,
// locale and decimal precision as the on-screen views. `currency` is deliberately
// required: it used to default to "USD" and no caller ever passed one, so every
// printed quotation and invoice rendered one fixed currency regardless of what it was issued in.
function fmtMoney(n: number, currency: string | undefined): string {
  return formatCurrency(n, currency || "USD");
}

// ─── Quotation builder ────────────────────────────────────────────────────────

export interface QuotationPrintData {
  number: string;
  /** ISO-4217 code the document was issued in; drives every money figure below. */
  currency?: string;
  title?: string;
  issue_date: string;
  valid_until?: string;
  status: string;
  customer_org?: { name: string; email?: string };
  customer_contact?: { full_name: string; email?: string };
  prepared_by?: { full_name?: string; email: string };
  items: {
    id?: string;
    /** Add-ons print indented under this line. */
    parent_item_id?: string | null;
    /** The line's snapshot SKU; product.sku is the fallback for older lines. */
    sku?: string | null;
    name?: string;
    description: string;
    product?: { name: string; sku?: string; terms_note?: string | null };
    qty: number;
    unit_price: number;
    discount_pct: number;
    tax_pct: number;
    tax_code?: string | null;
    subtotal: number;
    is_optional?: boolean;
    sale_model?: string | null;
    price_metric?: string | null;
    rate_period_months?: number | null;
    billing_interval_months?: number | null;
    term_months?: number | null;
  }[];
  subtotal: number;
  discount_amount?: number;   // the API name for the header discount (quotations; invoices use discount_total)
  tax_amount: number;
  /** Tax per rate: [{code, name, pct, amount}]. */
  tax_lines?: { code?: string | null; name?: string | null; pct?: number; rate?: number; amount: number }[] | null;
  total: number;
  notes?: string;
  terms?: string;
  customer_po?: string;
  /** 4 Nov 2026: optional lines are not in the total (an older quotation's stored total may still count them). */
  optional_subtotal?: number;
  optional_tax?: number;
  optional_total?: number;
  optional_in_total?: boolean;
}

export async function printQuotation(q: QuotationPrintData): Promise<void> {
  const company = await fetchCompany();
  const tree = orderLineTree(q.items.map((it, i) => ({ ...it, key: it.id ?? `i${i}`, parent_key: it.parent_item_id ?? null })));
  // 4 Nov 2026: optional lines print in their own section under the totals (as the server PDF does)
  const apart = (it: { is_optional?: boolean }) => !!it.is_optional && !q.optional_in_total;
  const optionalRows = tree.filter(({ line }) => apart(line)).map(({ line: item, depth }) => `
    <tr>
      <td style="padding-left:${12 + depth * 18}px">
        <div style="font-weight:600;color:#111">${depth ? "↳ " : ""}${escHtml(item.name || item.product?.name || "")}</div>
        ${item.description ? `<div style="color:#555;margin-top:2px">${escHtml(item.description)}</div>` : ""}
      </td>
      <td class="right">${item.qty}</td>
      <td class="right">${fmtMoney(item.unit_price, q.currency)}</td>
      <td class="right"><strong>${fmtMoney(item.subtotal, q.currency)}</strong></td>
    </tr>`).join("");
  const optionalTax = q.optional_tax ?? 0;
  const optionalSection = optionalRows ? `
<div class="section-title">Optional items (not included in the total)</div>
<table class="items">
  <thead><tr><th>Description</th><th class="right">Qty</th><th class="right">Unit Price</th><th class="right">Amount</th></tr></thead>
  <tbody>${optionalRows}</tbody>
</table>
<div class="totals">
  <div class="row"><span>Optional items subtotal</span><span>${fmtMoney(q.optional_subtotal ?? 0, q.currency)}</span></div>
</div>
<div style="font-size:10px;color:#777;text-align:right;margin-top:4px">Not included in the total above.${optionalTax > 0
    ? ` Tax on these items if taken: ${fmtMoney(optionalTax, q.currency)} (with tax: ${fmtMoney(q.optional_total ?? 0, q.currency)}).` : ""}</div>` : "";
  const rows = tree.filter(({ line }) => !apart(line)).map(({ line: item, depth }) => {
    const sku = item.sku || item.product?.sku;
    const term = lineTermText(item);
    const note = item.product?.terms_note;
    const taxLabel = item.tax_pct > 0
      ? `${item.tax_code && item.tax_code !== "CUSTOM" ? `${escHtml(item.tax_code)} ` : ""}${item.tax_pct}%` : "-";
    return `
    <tr>
      <td style="padding-left:${12 + depth * 18}px">
        <div style="font-weight:600;color:#111">${depth ? "↳ " : ""}${escHtml(item.name || item.product?.name || "")}${item.is_optional ? ' <span style="font-size:10px;color:#9ca3af;font-weight:400">(optional)</span>' : ""}</div>
        ${sku ? `<div class="sku">${escHtml(sku)}</div>` : ""}
        ${term ? `<div style="color:#6b7280;font-size:11px">${escHtml(term)}</div>` : ""}
        ${item.description ? `<div style="color:#555;margin-top:2px">${escHtml(item.description)}</div>` : ""}
        ${note ? `<div style="color:#6b7280;font-size:11px;font-style:italic;margin-top:2px;white-space:pre-line">${escHtml(note)}</div>` : ""}
      </td>
      <td class="right">${item.qty}</td>
      <td class="right">${fmtMoney(item.unit_price, q.currency)}</td>
      <td class="right">${item.discount_pct > 0 ? `${item.discount_pct}%` : "-"}</td>
      <td class="right">${taxLabel}</td>
      <td class="right"><strong>${fmtMoney(item.subtotal, q.currency)}</strong></td>
    </tr>`;
  }).join("");
  const taxRows = (q.tax_lines ?? []).length
    ? (q.tax_lines ?? []).map((t) => {
        const pct = t.pct ?? t.rate;
        return `<div class="row tax"><span>${escHtml(t.name || t.code || "Tax")}${pct != null ? ` (${pct}%)` : ""}</span><span>${fmtMoney(t.amount, q.currency)}</span></div>`;
      }).join("")
    : (q.tax_amount > 0 ? `<div class="row tax"><span>Tax</span><span>${fmtMoney(q.tax_amount, q.currency)}</span></div>` : "");

  const content = `
<div class="doc-header">
  <div>
    <div class="doc-title">QUOTATION</div>
    <div class="doc-number">${escHtml(q.number)}</div>
    ${q.title ? `<div style="margin-top:4px;font-size:14px;font-weight:600;color:#374151">${escHtml(q.title)}</div>` : ""}
  </div>
  <div class="doc-meta" style="text-align:right">
    <div><strong>Date:</strong> ${fmtDate(q.issue_date)}</div>
    ${q.valid_until ? `<div><strong>Valid until:</strong> ${fmtDate(q.valid_until)}</div>` : ""}
    ${q.customer_po ? `<div><strong>Customer PO:</strong> ${escHtml(q.customer_po)}</div>` : ""}
  </div>
</div>

${q.customer_org || q.customer_contact ? `
<div class="customer-block">
  <div class="label">Quotation Prepared For</div>
  ${q.customer_org ? `<div class="name">${escHtml(q.customer_org.name)}</div>` : ""}
  ${q.customer_contact ? `<div class="detail">${escHtml(q.customer_contact.full_name)}${q.customer_contact.email ? ` · ${escHtml(q.customer_contact.email)}` : ""}</div>` : ""}
  ${q.customer_org?.email && !q.customer_contact ? `<div class="detail">${escHtml(q.customer_org.email)}</div>` : ""}
</div>` : ""}

<table class="items">
  <thead>
    <tr>
      <th>Description</th>
      <th class="right">Qty</th>
      <th class="right">Unit Price</th>
      <th class="right">Disc</th>
      <th class="right">Tax</th>
      <th class="right">Subtotal</th>
    </tr>
  </thead>
  <tbody>${rows}</tbody>
</table>

<div class="totals">
  <div class="row"><span>Subtotal</span><span>${fmtMoney(q.subtotal, q.currency)}</span></div>
  ${(q.discount_amount ?? 0) > 0 ? `<div class="row discount"><span>Discount</span><span>−${fmtMoney(q.discount_amount!, q.currency)}</span></div>` : ""}
  ${taxRows}
  <div class="row total"><span>Total</span><span>${fmtMoney(q.total, q.currency)}</span></div>
</div>
${optionalSection}

${q.notes ? `<div class="section-title">Notes</div><div class="notes">${sanitizeHtml(q.notes)}</div>` : ""}
${q.terms ? `<div class="section-title">Terms &amp; Conditions</div><div class="notes">${escHtml(q.terms)}</div>` : ""}

<div class="signature-block">
  <div class="signature-inner">
    <div class="signature-line"></div>
    <div class="signature-name">${escHtml(company.company_signatory || (q.prepared_by?.full_name ?? q.prepared_by?.email ?? ""))}</div>
    <div class="signature-title">${escHtml(company.company_signatory_title || "Authorized Signatory")}</div>
  </div>
</div>

<div class="footer-note">
  This document was generated by ${escHtml(company.company_trade_name || company.company_name)} · ${escHtml(q.number)}
</div>`;

  await printDocument({ title: `Quotation ${q.number}`, contentHTML: content });
}

// ─── Invoice builder ──────────────────────────────────────────────────────────

export interface InvoicePrintData {
  number: string;
  /** ISO-4217 code the document was issued in; drives every money figure below. */
  currency?: string;
  invoice_type: string;
  status: string;
  issue_date: string;
  due_date?: string;
  customer_org?: { name: string; email?: string | null } | null;
  customer_contact?: { full_name: string; email?: string | null } | null;
  quotation?: { number: string };
  items: {
    name?: string;
    description: string;
    qty: number;
    unit_price: number;
    discount_pct: number;
    tax_pct: number;
    subtotal: number;
  }[];
  subtotal: number;
  discount_total?: number;
  tax_amount: number;
  total: number;
  amount_paid: number;
  outstanding: number;
  payments?: { amount: number; payment_date: string; method: string; reference?: string }[];
  notes?: string;
}

export async function printInvoice(inv: InvoicePrintData): Promise<void> {
  const company = await fetchCompany();
  const typeLabel = inv.invoice_type === "proforma" ? "PROFORMA INVOICE" : "COMMERCIAL INVOICE";
  const statusClass = inv.status === "paid" ? "status-paid" : inv.status === "overdue" ? "status-overdue" : inv.status === "sent" ? "status-sent" : "status-draft";

  const rows = inv.items.map((item) => `
    <tr>
      <td>
        <div style="font-weight:600;color:#111">${escHtml(item.name || item.description || "")}</div>
        ${item.name && item.description ? `<div style="color:#555;margin-top:2px">${escHtml(item.description)}</div>` : ""}
      </td>
      <td class="right">${item.qty}</td>
      <td class="right">${fmtMoney(item.unit_price, inv.currency)}</td>
      <td class="right">${item.discount_pct > 0 ? `${item.discount_pct}%` : "-"}</td>
      <td class="right">${item.tax_pct > 0 ? `${item.tax_pct}%` : "-"}</td>
      <td class="right"><strong>${fmtMoney(item.subtotal, inv.currency)}</strong></td>
    </tr>`).join("");

  const paymentRows = (inv.payments ?? []).map((p) => `
    <div class="payment-row">
      <span>${fmtDate(p.payment_date)} · ${p.method.replace("_", " ")}</span>
      <span style="font-weight:700;color:#16a34a">${fmtMoney(p.amount, inv.currency)}</span>
    </div>`).join("");

  const content = `
<div class="doc-header">
  <div>
    <div class="doc-title">${typeLabel}</div>
    <div class="doc-number">${escHtml(inv.number)}</div>
    ${inv.quotation ? `<div style="font-size:12px;color:#9ca3af;margin-top:2px;font-family:Arial,sans-serif">Ref: ${escHtml(inv.quotation.number)}</div>` : ""}
  </div>
  <div style="text-align:right">
    <div style="margin-bottom:6px"><span class="status-badge ${statusClass}">${inv.status.toUpperCase()}</span></div>
    <div class="doc-meta">
      <div><strong>Issue date:</strong> ${fmtDate(inv.issue_date)}</div>
      ${inv.due_date ? `<div><strong>Due date:</strong> ${fmtDate(inv.due_date)}</div>` : ""}
    </div>
  </div>
</div>

${inv.customer_org || inv.customer_contact ? `
<div class="customer-block">
  <div class="label">Bill To</div>
  ${inv.customer_org ? `<div class="name">${escHtml(inv.customer_org.name)}</div>` : ""}
  ${inv.customer_contact ? `<div class="detail">${escHtml(inv.customer_contact.full_name)}${inv.customer_contact.email ? ` · ${escHtml(inv.customer_contact.email)}` : ""}</div>` : ""}
</div>` : ""}

<table class="items">
  <thead>
    <tr>
      <th>Description</th>
      <th class="right">Qty</th>
      <th class="right">Unit Price</th>
      <th class="right">Disc</th>
      <th class="right">Tax</th>
      <th class="right">Subtotal</th>
    </tr>
  </thead>
  <tbody>${rows}</tbody>
</table>

<div class="totals">
  <div class="row"><span>Subtotal</span><span>${fmtMoney(inv.subtotal, inv.currency)}</span></div>
  ${(inv.discount_total ?? 0) > 0 ? `<div class="row discount"><span>Discount</span><span>−${fmtMoney(inv.discount_total!, inv.currency)}</span></div>` : ""}
  ${inv.tax_amount > 0 ? `<div class="row tax"><span>Tax</span><span>${fmtMoney(inv.tax_amount, inv.currency)}</span></div>` : ""}
  <div class="row total"><span>Grand Total</span><span>${fmtMoney(inv.total, inv.currency)}</span></div>
  ${inv.amount_paid > 0 ? `<div class="row" style="color:#16a34a"><span>Amount Paid</span><span>−${fmtMoney(inv.amount_paid, inv.currency)}</span></div>` : ""}
  ${inv.outstanding > 0 ? `<div class="row" style="font-weight:700;color:#dc2626;font-size:14px;border-top:1px solid #fecaca;padding-top:6px;margin-top:4px"><span>Outstanding</span><span>${fmtMoney(inv.outstanding, inv.currency)}</span></div>` : ""}
</div>

${(inv.payments ?? []).length > 0 ? `
<div class="section-title">Payment History</div>
<div class="payment-history">${paymentRows}</div>` : ""}

${inv.notes ? `<div class="section-title">Notes</div><div class="notes">${sanitizeHtml(inv.notes)}</div>` : ""}

<div class="signature-block">
  <div class="signature-inner">
    <div class="signature-line"></div>
    <div class="signature-name">${escHtml(company.company_signatory)}</div>
    <div class="signature-title">${escHtml(company.company_signatory_title)}</div>
  </div>
</div>

<div class="footer-note">
  This document is a valid ${typeLabel.toLowerCase()} issued by ${escHtml(company.company_name)}${company.company_trade_name ? ` (${escHtml(company.company_trade_name)})` : ""} · ${escHtml(inv.number)}
</div>`;

  await printDocument({ title: `${typeLabel} ${inv.number}`, contentHTML: content });
}
