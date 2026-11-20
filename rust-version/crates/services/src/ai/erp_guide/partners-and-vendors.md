# Procurement (vendors, RFQs, POs, receipts, vendor invoices) and partners

Procurement buys what the company needs from vendors and contractors; partner management runs the resellers who sell the company's products (deal registrations, commissions, MDF).

**Procurement** (/vendor-mgmt/...)
- Vendors: onboarding, active, suspended, terminated (vendor_mgmt:manage_vendors).
- RFQs: draft → sent (vendors invited) → evaluating (responses) → closed (can create a draft PO from the chosen vendor) or cancelled.
- Purchase orders: draft, sent, confirmed, partially_received, received, cancelled; creating needs vendor_mgmt:create_purchase_order. Sending passes the approval gate (team lead up to the policy limit, Owner above; two vendor quotes above a second limit). Goods are received only against an approved PO: one that skipped approval is submitted when someone tries to receive it, and receiving waits for the approver.
- Goods receipts: against a sent, confirmed or partially received PO, within `procurement.receipt_over_tolerance_pct`; they add stock and move the PO to partially received or received; a 3-way match view compares PO, receipt and invoice.
- Vendor invoices: submitted → under_review (submitted for approval: Finance Admin up to 25M, Owner above, needs a closed WO/PO) → approved or rejected → paid; approving and paying post to the ledger (vendor_mgmt:process_vendor_invoice).
- Contractor requests (vendor_mgmt:request_contractor) go to the Owner; work orders (draft → active → paused/completed/cancelled) are activated with vendor_mgmt:approve_work_order through the gate; timesheets are submitted, approved, rejected, paid.

**Partners** (/partner-mgmt/...)
- Partner accounts: tier authorized, silver, gold, platinum; status pending_agreement, active, suspended, terminated (partner_mgmt:manage).
- Deal registrations: pending → approved (links or creates a CRM deal) or rejected; then closed won (adds to the partner's revenue), closed lost, expired or extended (partner_mgmt:approve_deals). Nary watches registration windows and conflicts.
- Commissions: pending → approved → paid (or cancelled); approving needs partner_mgmt:approve_commission, paying partner_mgmt:pay_commission.
- MDF requests and claims: submitted → approved or rejected → paid.
