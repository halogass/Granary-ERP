# Sales

<!-- nav:top -->
[Home](Home) › Using Granary › Sell › **Sales**

**On this page:** [AI in this module](#ai-in-this-module) · [Quotations](#quotations) · [Solution proposals](#solution-proposals) · [Contracts](#contracts) · [Product catalog and price lists](#product-catalog-and-price-lists) · [Settings that affect this module](#settings-that-affect-this-module) · [Permissions](#permissions)
<!-- /nav:top -->

The Sales module turns CRM deals into priced, approved documents your customers can accept: quotations,
solution proposals and contracts. It also holds the product catalog with its price lists, which is the single
source of every quoted price. Sales staff write quotations and proposals, the Owner approves them, and finance
turns an accepted quotation into an invoice.

In the examples, Kumo Trading Co. (functional currency JPY) quotes office equipment and supplies to customers in
Japan and abroad.

![Quotation detail](images/sales-quotation-detail.png)

---

## AI in this module

Nary drafts quotations, proposals and contracts from conversations and chat requests, but it never sets a price
itself: catalog lines are priced by the same pricing service as the forms, and every draft goes through the normal
approval. See [AI Across Granary](AI-Across-Granary).

![A quotation draft card in the Nary chat](images/sales-ai-quotation-card.png)

- **Quotation drafts from the chat**: ask, for example, "Quote Hoshino Design Office 12 ergonomic chairs and 2
  standing desks, valid 30 days". Nary finds the products in the catalog and shows a draft quotation card with the
  priced lines. Click **Create** to save it as a draft. Manual and quote-only items are left for you to price.

  ![Asking Nary for a quotation and creating the draft from its card](images/sales-ai-quotation-draft.gif)

- **Change a quotation from the chat**: update a draft (deal, contact, lines, subject, notes), reprice it to
  today, withdraw it from approval, cancel it, make a new revision of a sent one, or link any quotation to a deal.
  Each is a card.
- **Quotations from customer conversations** (Nary on its own): when a customer asks for a quote, Nary picks the
  catalog lines at list price and proposes the draft as a card. Once your approval is final it proposes sending it.
  After sending, it drafts follow-ups on a schedule (`nary.cadence.steps`), skipping a step when the customer wrote
  recently. Each follow-up is a card.
- **Solution proposals**: for a complex deal Nary proposes requirement discovery (a card), keeps a requirement
  sheet on the deal, and then drafts the proposal with its charts, diagrams and an internal cost sheet. In the
  editor, type in **Ask Nary to change** (or **Ask Nary** on a figure) to have it rewrite sections or figures.
  When you confirm the proposal, Nary drafts the matching quotation.
- **Acceptance to contract**: when the customer accepts, Nary proposes the contract draft (a card), which then
  goes through the contract's own approval, and the down-payment proforma (another card).
- **Rate card research**: in **Settings > Sales & CRM > Cost breakdown & rate card**, click **Ask Nary to
  research current rates**. Each proposed rate change is a card for the Owner.
- **Catalog changes**: the Owner can ask Nary to change prices, add or retire products, or add product files.
  Each change is a card only the Owner confirms.

Costs, margins, volume tiers and negotiation hints stay internal: they never reach the model when Nary writes to a
customer.

---

## Quotations

Open **Sales > Quotations**. The saved view tabs are **All**, **Drafts**, **Sent**, **Accepted** and **PO signed**.
The cards on top show **Sent, pending**, **Won, accepted**, **Draft** and **Expiring soon**. Use **Search
quotations** to find one.

### Quotation statuses

| Status | Meaning |
|---|---|
| Draft | Being written. The only status you can edit. |
| Approved | A draft whose approval is complete; ready to send. |
| Sent | Sent to the customer. |
| Accepted | The customer said yes. |
| PO signed | You recorded the customer's purchase order. |
| Rejected | The customer said no. |
| Expired | The validity ended without an answer. Set automatically a few days after **Valid until**. |
| Superseded | Replaced by a newer revision. |
| Cancelled | Withdrawn for good, kept for history and hidden from the other views. |

### Create a quotation

1. Click **New quotation**. From a deal, use **New quotation** on the deal page instead: the deal and customer are
   filled in.
2. Under **Client and details**, pick the organization and the **Contact person**, the **Deal** (or **No deal**),
   a **Subject or title**, the **Issue date**, **Valid until**, the **Currency** and the **Payment terms**. The
   validity and payment terms start from Settings.
3. If the currency is not your functional currency, the **Exchange rate** is filled in from the recorded rate for
   the issue date (**Refresh rate** fetches it again). It is used for reporting.
4. Set the **Procurement type** (Standard, Government, White label or OEM). Anything other than Standard always
   goes to the Owner.
5. Under **Line items**, click **Add line** and pick a catalog item (**Pick a catalog item**). Enter the **Qty** and,
   for subscriptions, the **Term**. The **Unit price** comes from the catalog price list for the quotation's
   currency and issue date. The server prices every line, so the totals you see are the totals that will be saved.
6. Write the notes (see [Notes and notes templates](#notes-and-notes-templates)).
7. Click **Create quotation**.

You can edit a draft later with **Edit**, and **Save changes**.

### Lines, discounts and tax

- **Custom lines**: a line without a catalog item is a custom line. Only staff with `sales:price_manual` can
  enter it, or price a catalog item that is quote-only. Such quotations go to approval.
- **Add-ons**: some items (care plans, extra modules, a white-label option) attach to a parent line. Click
  **Add-on** on the parent line and pick one. It is priced from the parent and keeps the parent's quantity when
  it is priced per unit.
- **Line discount**: enter **Disc %** and pick the **Discount type** (Volume tier, Bundle, Loyalty,
  Multi-year SaaS or Owner exception). Every discounted line must state its type. A volume discount up to the
  item's tier counts as list price and is approved automatically. Discounts need `sales:apply_discount`.
- **Quotation discount**: under the subtotal, pick a **Discount type** (None or %), the **Discount value** and a
  **Discount note** (printed with the discount). **How the PDF shows the discount** chooses between a Discount line
  with the note, or notes only with the totals kept full.
- **Tax**: each line has a **Tax**: the customer's default, **No tax**, or **Custom %**. A customer marked
  **Quote without tax** in the CRM starts with no tax. The **Prices exclude tax** switch sets every line to no tax
  and adds the matching sentence to the notes.
- **Proposed price**: an item whose catalog price is only proposed shows **Proposed price**; any quotation with it
  goes to the Owner.

When a quotation has manual prices or discounts, the form says "Manual prices or discounts: this quotation goes to
approval."

### Optional lines

Tick **Optional** on a line to offer it without selling it. Optional lines:

- are shown on the PDF in their own section,
- stay out of the subtotal, tax and total,
- are summed separately under the total (**Optional items (not in the total)**, **Their tax**, **Total with
  optional items**),
- are never invoiced.

### Notes and notes templates

The **Notes, terms and conditions** card holds the text printed under the lines.

- **Fill notes from template** replaces the notes with the house template of the lines' product family (products,
  services or subscriptions). Add the lines first: the template follows them. The template fills the standard
  clauses (payment, delivery, warranty and so on); the deal-specific clauses (scope, delivery details) you still
  write yourself.
- The notes are checked as you type. A panel lists problems: missing required clauses, notes that are not
  numbered, a discount written in the notes by someone without the pricing permission, or tax wording that does
  not match the lines (for example "prices exclude tax" while a line carries tax). If the panel says **The approval
  will refuse these notes**, fix them before you submit. When everything is fine it says "The notes follow the
  house template."
- **Insert saved note** inserts a named snippet from the shared library; **Save as template** saves the current
  notes as a new snippet. Snippets marked **shared** are available on every document type. The person who saved a
  snippet, the Owner and system admins can delete it.

Sent or approved quotations are never re-checked.

### Pricing rules

On a saved draft, the **Pricing check** panel shows the pricing rules: a price below the minimum multiple of cost
blocks the approval, and other findings only warn. Only the Owner can let a blocked draft through: **Override
(Owner)**, with a reason. The override holds for the lines as they are now and is recorded in the audit log. An
empty reason removes it.

### Send a quotation

Click **Send to client**.

- If the quotation is not approved yet, it is submitted for approval. Catalog items at list price with no discount
  are approved on the spot; anything else waits for the Owner. You are told it now waits for approval. Follow it in
  the **Approval** panel on the page or in [Approvals](Approvals-and-Signing).
- Once approved, the send dialog opens with the PDF and its verification QR attached. See
  [Sending documents](Approvals-and-Signing#send-an-approved-document).

While a quotation waits for approval, **Withdraw from approval** takes it back so you can change it (the person
who submitted it, or the Owner).

### After sending

On a sent quotation:

- **Resend** sends it again. The number does not change.
- **Mark accepted** or **Mark rejected** records the customer's answer.
- **Revise** creates a new revision (see below).

On an accepted quotation:

- **Record PO** saves the customer's **PO number**; the status becomes **PO signed**.
- **Convert to invoice** creates the invoice (see below).

Use **Remind me** to set a follow-up, and **PDF** to download the verified copy. **Link to deal** (or **Change
deal**) links an issued quotation, and all its revisions, to a deal without a revision; the customer's document
does not change.

### Revisions

To change a quotation that has been sent, rejected or has expired, click **Revise** and give a **Reason for
revision**. The current quotation becomes **Superseded** and an editable draft opens with the next revision number
(the number's last part counts the revisions: -1, -2, -3 and so on). The **Revisions** card lists every revision, and a superseded one links to
**Go to current revision**.

### Reprice, cancel, delete

- **Reprice to today** (draft): the issue date becomes today, the validity counts from today, catalog lines take
  today's catalog price and typed lines keep theirs. The quotation then goes to approval again. Each line's price
  before and after is shown.
- **Cancel quotation** (draft, also one waiting for approval): give a reason. Its approval request is withdrawn,
  it can never be sent and it is kept for history.
- **Delete** (from the list): only for draft, rejected or expired quotations.

### Payment schedule and quotation to invoice

Under **Payment schedule**, click **Add milestones** to split the quotation into installments. For each, enter a
**Milestone name**, a **Percentage** and a **Due trigger** (**On order**, **On delivery**, **On milestone** or
**On date**), then **Save schedule**.

To bill, click **Convert to invoice** (or **Generate invoice** next to a milestone):

1. Optionally set a **Payment due date**.
2. Pick **Full invoice** (all non-optional lines, with the quotation discount) or a milestone.

Granary opens the new draft invoice in Finance. Each line keeps the tax set on the quotation. An invoice never
bills more than the quotation is worth: optional lines are left out and earlier invoices count. Only sent,
accepted or PO-signed quotations can be invoiced, and you need `finance:create_invoice`. See [Finance](Finance).

![Create invoice from a quotation](images/sales-quotation-to-invoice.gif)

---

## Solution proposals

A solution proposal is a longer document for custom or complex deals: an explanation of the solution with its
quotation(s), approved and sent together. Open **Sales > Proposals** (**Solution proposals**). The saved views are
**All**, **Drafts**, **In review**, **Approved**, **Sent**, **Rejected** and **Drafted by Nary**.

### Create a proposal

- From a deal: click **Solution proposal** on the deal page, then enter a title and click **Create**.
- From the list: enter a title under **New proposal**, pick the **Language** and click **Create**.
- [Nary](Nary-AI-Assistant) can also draft one from requirement discovery. Those show "by Nary".

### Edit a proposal

Open the proposal. The editor has the sections from your proposal template. You can:

- edit text blocks (Markdown), tables (with **Requirement**, **Status** and **Notes** columns) and images
  (**Upload** an image, add a **Caption**, pick full or half width),
- reorder blocks (**Move up**, **Move down**) and remove them,
- add figures (charts and diagrams),
- type a request in **Ask Nary to change** (for example "make the implementation plan 8 weeks and add a pilot
  phase") and click **Ask**. Long changes can take a few minutes; the page refreshes when Nary is done.
- click **Save** to keep your changes, and **Preview PDF** to see a draft PDF.

The commercial summary is generated from the linked quotation(s) when the PDF is made, so its numbers always
match; you cannot type it. The approval stamp and QR are added once the proposal is approved.

### Confirm prices and get the quotation

Staff with `sales:view_costs` see the internal **cost sheet** with the estimated labour and the suggested price at
the target margin (**Re-estimate** recalculates it). It is never shown to the customer.

While the proposal has no quotation, the **Confirm the proposal** panel asks for your final prices: catalog items
and custom lines with prices. Confirm, and Nary drafts the matching quotation, linked to the proposal, for
approval. The proposal must belong to a deal. You can also link an existing quotation.

### Approve, send, revise

- The **Approval** panel works as for quotations. The Owner approves every proposal by default; any edit after
  approval needs approval again.
- **Send with its quotation(s)** sends the approved proposal and its quotations together.
- **Revise** opens a new revision; the reason is printed on the cover.
- **Reject** makes a draft or in-review proposal read-only and stops Nary working on it. **Reopen** turns a
  rejected proposal back into a draft.

---

## Contracts

Open **Sales > Contracts** for sales agreements, MSAs, NDAs and service contracts. The saved views are **All**,
**Drafts**, **Sent for signing** and **Signed**. The cards show **Total**, **Signed**, **In progress** and
**Total value**. Signed contracts that end soon are marked **expiring soon**.

### Create a contract

1. Click **New contract**.
2. Pick the **Customer**, the **Contract type** (sales, service, MSA, NDA, partner agreement or vendor agreement),
   the **Currency** and the **Contract value**.
3. Set the **Start date** and **End date**, tick **Auto-renew on expiry** if it renews, and add **Notes**.
4. Under **Documents**, drop the PDF and any supporting files (optional).
5. Click **Create contract**.

### Send and sign

1. On a draft, click **Send**. The Owner reviews every contract before it goes to the customer, so the first
   click submits it for approval. After approval, the contract moves to **Sent for signing**.
2. With an approved contract you can collect the customer's signature electronically from the approval request
   (see [Approvals and Signing](Approvals-and-Signing#e-signature-for-contracts-and-offer-letters)).
3. When it is signed, click **Mark signed** and enter the **Customer signatory name** and **title**, **Our
   signatory name** and, optionally, a **Document URL**. Click **Confirm signed**.

Only draft or terminated contracts can be deleted.

---

## Product catalog and price lists

Open **Sales > Product Catalog**. Every quoted price comes from here. The views are **All items**, **One-time**,
**Recurring** and **Add-ons**; filter by **Family**, tick **Show retired**, or search by name or SKU.

Only the Owner and users with the Executive role can edit the catalog, because list prices decide whether a
quotation is approved automatically. Others see "Only catalog managers can edit the catalog." and can open
items with **View item**.

![Product catalog](images/sales-catalog.png)

### Add an item

1. Click **New item**.
2. Fill in **SKU**, **Name**, **Short name**, **Family** (for example `laser_printers`), **Category**, **Type**,
   **Unit label**, **Product line** and **Description**.
3. Under **How it is sold**, pick the **Sale model** (one-time or recurring), **Price per**, **Min quantity** and,
   for recurring items, **Price period (months)**, **Billed every (months)** and **Min term (months)**. **Care
   included (months)** is for items that come with a care period.
4. Under **Pricing**, pick the **Pricing mode**, the **Cost price** (internal) and the **Price status**:
   **Locked**, or **Proposed** (the Owner then approves every quotation that uses it). A manual pricing mode means
   staff with the manual-price permission type the price on each quotation.
5. Optional sections:
   - **Add-on**: the parent SKUs this item attaches to (care, extra modules, white-label). Leave empty for a
     standalone item.
   - **White-label**: whether this item is the white-label option, and how its price is set (not set, % of the
     parent price, or fixed from its own price list).
   - **Sizing**: **Unit equivalents** (for the payment-term split) and **Modules** included.
   - **Quotation text and status**: a **Terms note** printed under the line, **Active** and **Orderable**.
6. Save. Then add prices and tiers.

### Price lists

Each item has one list price per currency, effective from a date. A price review adds a new row with a later
**Valid from** instead of overwriting, so a quotation is always checked against the price of its own issue date.
Enter the **Currency**, the **Amount** and the **Valid from** date and add the row.

Currencies without their own list are converted from the catalog base currency (see the settings below).

### Volume tiers

Under **Volume tiers**, set ranges (**From** ... units: **% off**). Tiers are counted per SKU across the
quotation's non-optional lines. Without tiers, the list price applies at every quantity. A volume discount up to
the tier is treated as list price and approved automatically.

---

## Settings that affect this module

Under **Settings > Company & branding > Document numbers**: **Quotation prefix** (`quotation_prefix`).

Under **Settings > Sales & CRM**:

- **Quotations**:
  - **Quotation validity** (`quotation_validity_days`): days a new quotation stays valid.
  - **Expire after validity ends** (`sales.quotation.expire_grace_days`) and **Re-quote reminder window**
    (`sales.quotation.expired_nudge_days`).
  - **Discount without approval** (`max_discount_pct_without_approval`).
  - **Prices exclude tax: the sentence** (`sales.quotation.tax_exclusive_note`) and the phrases the notes check
    reads (`sales.quotation.tax_exclusive_phrases`, `sales.quotation.tax_inclusive_phrases`).
- **Quotation notes templates**: whether a failed notes check stops the approval
  (`sales.quotation.notes.block_submit`), delivery days, invoice terms, warranty years, the families and their
  templates (`sales.quotation.notes_template.*`), the payment and tax sentences, and what the check looks for.
- **Pricing rules**: **Stop the approval on a blocking pricing rule** (`sales.pricing_rules.block_submit`),
  **Minimum multiple of full cost** (`sales.pricing_rules.min_cost_multiple`) and **Always Owner approval**
  SKU patterns (`sales.pricing_rules.owner_approval_skus`).
- **Pricing & payment terms**: **Catalog base currency** (`catalog.conversion_base_currency`), **Currencies with
  their own price list** (`catalog.independent_currencies`), split payment and down payment suggestions, and the
  internal target and floor margins (`sales.margins`).
- **Proposals**: the proposal sections, fixed words, validity, discovery checklist, default and required
  figures, and which sections may state prices.
- **Cost breakdown & rate card**: contingency, working days per month, employer load factor, minimum monthly
  wage, and the **Rate card** (cost per man-day by role and seniority, internal).

Under **Settings > Approvals & signing > Approval policies**: the quotation, proposal and contract approval chains
and the **Discount allowances** (see [Approvals and Signing](Approvals-and-Signing)).

---

## Permissions

| Permission | Allows |
|---|---|
| `sales:view_quotations` | See quotations and proposals |
| `sales:create_quotation` | Create and change quotations and proposals, revise, reprice, cancel, link to a deal |
| `sales:edit_quotation` | Delete draft, rejected or expired quotations |
| `sales:send_quotation` | Send quotations and proposals to customers |
| `sales:apply_discount` | Give line and quotation discounts |
| `sales:price_manual` | Enter manual prices (custom lines, quote-only items, off-list prices) |
| `sales:view_costs` | See cost sheets and the rate card |
| `sales:manage_costs` | Change the rate card |
| `sales:view_contracts` | See contracts |
| `sales:create_contract` | Create, send, sign and delete contracts |
| `sales:view_catalog` | See the catalog and its prices |
| `finance:create_invoice` | Turn a quotation into an invoice |

Editing the catalog is limited to the Owner and the Executive role. The Owner (superadmin) passes every check;
the pricing override is Owner-only.

---

## Related pages

- [CRM](CRM): deals, organizations and contacts
- [Approvals and Signing](Approvals-and-Signing): approval, verification QR, e-signature, sending
- [CRM Inboxes](CRM-Inboxes): where sent documents and replies appear
- [Finance](Finance): invoices made from quotations
- [Multi-Currency](Multi-Currency): exchange rates on quotations
- [R&D](RnD): product lines
- [Nary AI Assistant](Nary-AI-Assistant): drafting proposals and quotations

<!-- nav:bottom -->
---

← Previous: [CRM Inboxes](CRM-Inboxes) | Next: [Marketing](Marketing) →
<!-- /nav:bottom -->
