# Procurement

<!-- nav:top -->
[Home](Home) › Using Granary › Buy and make › **Procurement**

**On this page:** [AI in this module](#ai-in-this-module) · [Find your way around](#find-your-way-around) · [Vendors](#vendors) · [Contractor requests](#contractor-requests) · [Requests for quotation (RFQs)](#requests-for-quotation-rfqs) · [Purchase orders](#purchase-orders) · [Goods receipts](#goods-receipts) · [Three-way match](#three-way-match) · [Work orders](#work-orders) · [Vendor invoices](#vendor-invoices) · [Vendor portal](#vendor-portal) · [Settings that affect this module](#settings-that-affect-this-module) · [Permissions](#permissions)
<!-- /nav:top -->

The Procurement module covers everything you buy: the vendors and contractors you work with, requests for a
contractor, requests for quotation (RFQs), purchase orders, goods receipts, work orders for services, and the
invoices vendors send you, with withholding tax where it applies. Vendors and contractors have their own
**vendor portal**, where they answer RFQs, see their work orders, log hours, upload compliance documents and
submit invoices. Purchasing and operations staff run the module; finance approves and pays vendor invoices.

![The purchase order list with committed spend per currency](images/procurement-purchase-orders.png)

---

## AI in this module

Nary has no procurement tools yet: it can't read or change vendors, RFQs, purchase orders or vendor invoices. In the chat it can explain how this module works, from the ERP guide built into Granary. See [AI Across Granary](AI-Across-Granary) for what the AI does in the other modules.

---

## Find your way around

Open **Vendors** in the sidebar. It has these pages:

| Page | What it is for |
|---|---|
| **Vendor List** | Vendors and contractors, their contact details, rating and portal access |
| **Contractor Requests** | Internal requests to hire a contractor, with their approval |
| **Work Orders** | Service engagements with a contractor (scope, agreed value, dates) |
| **Purchase Orders** | Orders for goods |
| **Vendor Invoices** | Invoices that vendors submit, their review, approval and payment |
| **RFQs** | Requests for quotation sent to several vendors |
| **Goods Receipts** | Goods received against a purchase order |

The usual flow for goods is: **RFQ > purchase order > approval > send to vendor > goods receipt > vendor invoice
> payment**. For services: **contractor request > work order > approval > send to contractor > vendor invoice >
payment**.

---

## Vendors

### Add a vendor

1. Open **Vendors > Vendor List** and click **Add vendor**.
2. Enter the **Company name** and pick the **Type**: Professional Services, IT Infrastructure, Component
   Supplier, Assembly Subcontractor, Facility Services, Logistics / Freight, Temporary Staffing or Professional
   Non-Technical.
3. Enter the **Contact name**, **Contact email**, **Country** and **Tax ID**.
4. Click **Add vendor**.

New vendors start in onboarding. The tabs are **All**, **Active** and **Top rated**; the tiles count the total,
active and top-rated vendors and the types in use.

### Give a vendor portal access

In the vendor list, click **Grant portal access** on the vendor's row. Granary creates a portal account and
emails the sign-in details to the vendor's contact email. The row then shows **Invite sent** or **Has access**.
A vendor with no contact email shows **No email on file**: add one first.

Vendors can also ask for access themselves from the portal sign-in page (see
[Vendor portal](#vendor-portal)). An administrator approves those requests under **Portal users**; see
[Administration](Administration).

---

## Contractor requests

A contractor request is an internal request to bring in a contractor. It goes through approval before anyone
is hired.

1. Open **Vendors > Contractor Requests** and click **New request**.
2. Enter the **Scope description**, the **Contractor type**, the **Department**, the **Needed by** date, the
   **Currency** and the **Estimated value**.
3. Click **Submit request**.

The request goes to its approval chain. An approver clicks **Approve**, or **Reject** with a **Rejection
reason**. The tabs are **All**, **Pending approval**, **Approved** and **Rejected**. Only pending, rejected or
cancelled requests can be deleted.

---

## Requests for quotation (RFQs)

An RFQ asks several vendors to quote for the same list of items. You then award the purchase order to one of
them.

![An RFQ with its line items and the vendors' quotes side by side](images/procurement-rfq-detail.png)

### Create and send an RFQ

1. Open **Vendors > RFQs** and click **New RFQ**.
2. Enter a **Title** (for example "Office chairs, Q3"), the **Currency** and the **Response deadline**.
3. Under **Line items**, click **Add line** for each item and enter the **SKU**, **Description**, **Qty** and
   **Unit**.
4. Click **Create RFQ**. The RFQ is a draft.
5. Open the RFQ, pick a vendor in **Vendor to invite** and click **Invite**. Repeat for each vendor.

Inviting the first vendor moves the RFQ from draft to sent. Invited vendors see it in the vendor portal and
submit their quotes there. Each line then shows the vendors' responses.

### Award the RFQ and create a purchase order

1. Open the RFQ and click **Close and create PO**.
2. In **Award to**, pick the winning vendor (only vendors who responded are offered).
3. Under **Select responses for PO lines**, tick the quotes to use.
4. Click **Create PO**.

Granary closes the RFQ and creates a draft purchase order for that vendor from the selected quotes. Only a sent
or evaluating RFQ can be closed. Every selected quote must be in the RFQ's currency and come from the awarded
vendor. Only draft or cancelled RFQs can be deleted.

![Creating an RFQ, inviting vendors and awarding the purchase order](images/procurement-rfq-to-po.gif)

---

## Purchase orders

### Create a purchase order

1. Open **Vendors > Purchase Orders** and click **New PO**.
2. Pick the **Vendor** and the **Currency**.
3. Under **Items**, click **Add line** for each item and enter the **Description**, **Qty** and **Unit price**.
4. Set the **Expected delivery** date and click **Create PO**.

A purchase order created from an RFQ is filled in for you.

### Approve and send

1. In the list, click **Send** on the order.
2. If the order is not approved yet, it is submitted to its approval chain and waits. Approvers decide it from
   **Approvals** (see [Approvals and Signing](Approvals-and-Signing)). A rejected order goes back to draft for
   rework.
3. Once it is approved, click **Send** again. Choose email or WhatsApp, check the recipient and click
   **Confirm & send**. The approved PDF with its verification QR goes to the vendor.

The tabs are **All**, **Drafts**, **Awaiting delivery** and **Received**, plus a **Cancelled** chip. The tiles
show committed spend, orders awaiting delivery, received orders and total spend. Spend is shown per currency:
draft and cancelled orders don't count. Only draft or cancelled orders can be deleted.

---

## Goods receipts

Record a receipt when goods arrive.

![Recording a goods receipt against an approved purchase order](images/procurement-goods-receipt.png)

1. Open **Vendors > Goods Receipts** and click **Record receipt**.
2. Pick the **Purchase order**. Only approved orders that are not fully received are offered.
3. Enter the **Receipt date** and, optionally, the location to **Store to location**.
4. Under **Items to receive**, enter the quantity received for each line. Enter at least one.
5. Add **Notes** and click **Record receipt**.

What Granary does:

- It refuses a receipt against an order whose approval is still pending, or one in a status that can't
  receive.
- It allows a small over-delivery, up to the **Over-delivery tolerance** (5% by default). Beyond that, the
  receipt is refused with the ordered and received quantities.
- It adds the received quantity to component inventory for every line with a SKU, and records a stock
  movement. See [Manufacturing and Inventory](Manufacturing-and-Inventory).
- It updates the purchase order's received quantities and status.

---

## Three-way match

For each purchase order, Granary compares what was **ordered**, what was **received** (goods receipts) and what
was **invoiced** (approved vendor invoices), line by line. Each line is:

| Status | Meaning |
|---|---|
| matched | Received and invoiced quantities both reach the ordered quantity |
| partial | Some quantity received or invoiced, but not all |
| over | More received or invoiced than ordered |
| unmatched | Nothing received or invoiced yet |

The order is matched overall when every line is matched. Invoiced amounts are totaled per currency. The match
is available from the API (`GET /api/v1/vendor/purchase-orders/{id}/matching`); there is no screen for it in
this version.

---

## Work orders

A work order is a service engagement with a contractor.

1. Open **Vendors > Work Orders** and click **New work order**.
2. Pick the **Vendor**, enter a **Title** (for example "Office network upgrade"), the **Scope**, the **Agreed
   value**, the **Currency**, the **Start date** and the **End date**.
3. Click **Create work order**. It starts as a draft.

Move it through its life with the buttons on its row:

| From | Actions |
|---|---|
| Draft | **Activate**, **Cancel** |
| Active | **Complete**, **Pause**, **Cancel** |
| Paused | **Resume**, **Cancel** |

Activating a work order needs the `vendor_mgmt:approve_work_order` permission and passes the work order's
approval policy. Click **Send** to send the approved work order to the contractor by email or WhatsApp, as for
purchase orders. Only draft or cancelled work orders can be deleted. The value tiles are per currency and
leave out cancelled orders.

---

## Vendor invoices

Vendors submit their invoices from the vendor portal, against one of their purchase orders or work orders.
You review, approve and pay them in **Vendors > Vendor Invoices**.

![The vendor invoice list with withholding tax and net payable](images/procurement-vendor-invoices.png)

Each invoice shows the **Amount**, the **Withholding** tax and the **Net payable** (amount + sales tax -
withholding tax). The tiles show the **Total**, **Pending**, **Payable** and **Withholding tax** totals.

To process an invoice:

1. Click **Review** on a submitted invoice. It goes to its approval chain (status under review).
2. Click **Approve**, or **Reject** with a **Rejection reason**. A rejected invoice can be reviewed again.
3. When you pay it, click **Mark paid**, choose **Paid from** (the bank account), the **Paid date** and a
   **Payment reference**.

Only an approved invoice can be paid, and only while its approval still matches the invoice. Only submitted or
rejected invoices can be deleted. With automatic posting on, the invoice posts to the ledger when it is
approved and again when it is paid (see [Finance](Finance)).

### Withholding tax

Withholding tax is the part of a vendor's invoice that you keep back and pay to the tax authority on the
vendor's behalf. Granary computes it when the invoice is submitted: the invoice amount times the vendor's
withholding rate, rounded to the currency. The vendor sees the withheld amount and the net payable in the
portal.

- A vendor that is marked as withholding but has no rate can't submit an invoice: the portal tells them to
  contact you.
- A work order can carry its own withholding rate; without one it uses the vendor's rate.
- The vendor list and the forms in this version don't include a field to mark a vendor as withholding or set
  its rate.

Withholding taxes for your ledger are configured in **Finance > Taxes** (types **Withholding: services** and
**Withholding: employees**).

Rules the server applies to every vendor invoice:

- It must be for an order of that vendor, and in the currency of that order.
- If it names both a purchase order and a work order, they must be in the same currency.
- The same invoice number can't be submitted twice.

---

## Vendor portal

The vendor portal is a separate sign-in for vendors and contractors. They see only their own records.

![The vendor portal: a vendor's invoices with withholding tax and net payable](images/procurement-portal-invoices.png)

### Sign in and request access

- Vendors sign in at `/vendor/login` with their email address and password (two-factor sign-in applies if it
  is turned on for the account).
- A new vendor clicks **Request portal access** and fills in the **Company name**, **Contact name**, **Phone**,
  **Email address**, **Vendor or contractor type**, **Country**, **Website** and **Services offered**, then
  clicks **Submit registration request**. An administrator approves the request under **Portal users**; the
  vendor then gets a welcome email.

### What a vendor can do

The portal menu has **Dashboard**, **Work orders**, **RFQs**, **Invoices**, **Timesheets** and **Documents**.

| Page | What the vendor does there |
|---|---|
| **Work orders** | Sees the work orders assigned to them, with dates and value |
| **RFQs** | Sees the RFQs they were invited to. Clicks **Quote now**, enters a **Unit price** for each line, the **Lead time (days)** and **Additional notes**, then **Submit quote**. A closed RFQ no longer accepts quotes |
| **Invoices** | Clicks **Submit invoice**, picks the **Purchase order or work order**, enters their **Invoice number**, the **Amount before tax**, any **Sales tax**, the **Invoice date**, the **Due date** and **Notes**. The list shows withholding tax, net payable and what has been paid |
| **Timesheets** | Clicks **Log hours** and enters the **Period start**, **Period end**, **Hours worked** and a **Work summary** |
| **Documents** | Clicks **Upload document**, picks the **Document type** (NDA, service agreement, certificate, insurance, license, tax registration or other), names it, adds the **Signed date** and **Expiry date**, and uploads the file. Each document shows **Verified** or **Pending review**; documents expiring within 30 days are flagged |

Uploaded documents must be PDF, PNG, JPEG, WebP, Word (.docx) or Excel (.xlsx), and no larger than the
**Largest vendor document** setting (10 MB by default). A verified document can't be deleted by the vendor.

---

## Settings that affect this module

Open **Settings > General**, then **Operations > Procurement**.

| Setting | Key | What it does |
|---|---|---|
| Over-delivery tolerance | `procurement.receipt_over_tolerance_pct` | How far (in %) a goods receipt may exceed the ordered quantity |
| Withholding tax rate for new vendors | `procurement.default_withholding_rate` | Percent for new vendors that withhold tax |
| Largest vendor document | `vendor.portal.max_upload_mb` | Largest file a vendor may upload in the portal (MB) |

Approval chains for purchase orders, work orders, contractor requests and vendor invoices are set under
**Approvals & signing**; see [Approvals and Signing](Approvals-and-Signing). Currencies are covered in
[Multi-Currency](Multi-Currency).

---

## Permissions

| Permission | Allows |
|---|---|
| `vendor_mgmt:view` | See vendors, RFQs, receipts and the three-way match |
| `vendor_mgmt:manage_vendors` | Add and delete vendors, grant portal access, run RFQs, record goods receipts |
| `vendor_mgmt:request_contractor` | Create and delete contractor requests |
| `vendor_mgmt:manage_rfq` | Create, invite, close and delete RFQs |
| `vendor_mgmt:create_purchase_order` | Create, delete and send purchase orders; run RFQs |
| `vendor_mgmt:approve_purchase_order` | Record goods receipts |
| `vendor_mgmt:create_work_order` | Create, change, delete and send work orders |
| `vendor_mgmt:approve_work_order` | Activate work orders |
| `vendor_mgmt:process_vendor_invoice` | Review, approve, reject, pay and delete vendor invoices |

Approving a purchase order, a contractor request or another document's approval step depends on the approval
policy, not on these permissions. Vendor portal users can only use the portal pages. Superadmins can do
everything.

---

## Related pages

- [Finance](Finance): payments, the ledger, taxes and withholding tax accounts
- [Multi-Currency](Multi-Currency): currencies and exchange rates
- [Manufacturing and Inventory](Manufacturing-and-Inventory): component inventory that goods receipts fill
- [Approvals and Signing](Approvals-and-Signing): approval policies and sending documents
- [Administration](Administration): portal users and registration requests

<!-- nav:bottom -->
---

← Previous: [Multi-Currency](Multi-Currency) | Next: [Manufacturing and Inventory](Manufacturing-and-Inventory) →
<!-- /nav:bottom -->
