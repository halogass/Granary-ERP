# Approvals and Signing

<!-- nav:top -->
[Home](Home) › Using Granary › Approve and sign › **Approvals and Signing**

**On this page:** [AI in this module](#ai-in-this-module) · [How approval works](#how-approval-works) · [The Approval panel on a document](#the-approval-panel-on-a-document) · [Approve or reject](#approve-or-reject) · [The approval stamp and the verification QR](#the-approval-stamp-and-the-verification-qr) · [Send an approved document](#send-an-approved-document) · [E-signature for contracts and offer letters](#e-signature-for-contracts-and-offer-letters) · [The Signing module](#the-signing-module) · [Set approval policies](#set-approval-policies) · [Settings that affect this module](#settings-that-affect-this-module) · [Permissions](#permissions)
<!-- /nav:top -->

Granary makes sure that nothing important leaves the company, or gets paid, without the right person's
approval. Each document type (quotation, proposal, contract, invoice, purchase order, expense, offer letter and
more) has an approval policy that decides who must approve it and when. An approved document carries the
approver's stamp and a verification QR on every page, so anyone who receives it can check it is genuine and
current. Contracts and offer letters can also be signed electronically by the other party, and any PDF can be
sent out for signing from the **Signing** module.

Approvers use the **Approvals** inbox; everyone else meets approvals in the **Approval** panel on their
documents. Administrators set the policies in Settings.

![Approvals inbox](images/approvals-inbox.png)

---

## AI in this module

Nary's work does not skip approvals. A quotation, proposal, contract or invoice that Nary drafts is an ordinary
draft and goes through the same approval policies as one you made. See [AI Across Granary](AI-Across-Granary).

- **Two kinds of approval**: Nary's own cards (send this reply, create this draft, merge these contacts) are
  decided in **Nary > Approvals**, or by the Owner replying with the card's number on WhatsApp. The documents
  those cards create then follow the approval policies on this page.
- **Sending**: when Nary sends a quotation, invoice or proposal to a customer, it attaches the QR-verified PDF
  exactly as **Send** does. A document that is not approved yet goes for approval first and the message waits.
- **Proposal figures**: a solution proposal can't be submitted for approval while a required figure (for example
  the timeline) is missing, has no image, or failed its check. The refusal lists what is missing.
- **Contracts**: the contract draft Nary proposes when a customer accepts gets the contract's own Owner approval.
- **SOP revisions**: SOP drafts written by Nary are signed and published through the normal SOP approval.

---

## How approval works

1. You submit a document. Usually this happens by itself: clicking **Send to client** on a quotation, **Send** on a
   contract, or scheduling marketing content submits it for you. You can also click **Submit for approval** in
   the document's **Approval** panel.
2. Granary checks the policy's **submission requirements** first. A document that fails one cannot be submitted;
   the panel lists the reasons (for example "Every discounted line must state its discount type").
3. The policy's steps are worked out for this document. A step can depend on conditions, such as "amount above N
   in the functional currency" or "not a catalog item at list price". A step whose amount cannot be worked out
   (for example, no exchange rate on record) applies rather than being skipped.
4. If no step applies, the document is approved at once. Otherwise each step goes to its approver in order.
5. When the last step approves, the document is **Approved** and can be sent.

Rules that always hold:

- A rejected document goes back to its author. When it is resubmitted, approval restarts from the first step.
- If an approved document is changed, the approval no longer matches ("The document changed after it was
  approved. Resubmit for approval.").
- An approver cannot approve their own submission unless the step allows it (or they are the Owner).
- Overdue steps are reminded at most once a day.

The shipped policies follow common practice and are meant to be adjusted. For example: quotations with only
catalog items at list price and no discount are approved automatically, and the Owner approves every other
quotation; the Owner approves every invoice, credit note, proposal and contract.

---

## The Approval panel on a document

Quotations, proposals, invoices, offer letters, SOP articles, library documents and bills of materials show an
**Approval** panel. It shows:

- the current state (for example waiting, approved, rejected),
- the **trail**: every step with its state (**Approved**, **Rejected**, **Waiting**, **Queued**, **Not reached**,
  **Informed**), who decided and when, who is expected to decide, the due time, and **Overdue** when it is late,
- comments left by approvers,
- **Submit for approval** or **Resubmit for approval** when that is possible. Some policies show tick boxes (flags)
  to answer before you submit, such as confirming a pre-approval,
- **Withdraw request**, for the person who submitted it, to take it back,
- the **Verified document** and **E-signature** sections (see below), once the document is approved.

---

## Approve or reject

Open **Approvals** in the sidebar (the badge counts what waits for you). The saved views are **Awaiting me**,
**Submitted by me** and **Everything I'm involved in**; the chips filter **Pending**, **Approved** and
**Rejected**. Use **Search approvals** to find a document.

1. Click a request to expand it. You see the document, who submitted it, the amount and the trail. **Open
   document** opens the document itself.
2. Optionally write a **Comment**.
3. Click **Approve**, or **Reject**. Rejecting needs a **Reason**; the document goes back to its author.

When a request is approved, the expanded row also offers the document's actions: **Send to client** (or **Send to
vendor**), the verified PDF and, where the policy uses it, e-signature. This is how you act on documents that have
no page of their own, such as contracts.

Who may decide a step is fixed when the document is submitted. The `approvals:view` permission only controls who
sees the inbox.

![Approve a quotation](images/approvals-approve.gif)

---

## The approval stamp and the verification QR

When a policy has **Print a verification QR** on, an approved document gets a QR code on every page and the
approver's stamp. The QR opens a public page at `/v/<token>`.

### Issue, download and revoke

In the **Verified document** section of the Approval panel:

- **Issue verified PDF** renders the approved document with the QR. Downloading a quotation's **PDF** after
  approval does the same.
- The section shows the document number, its state, when it was issued and expires, how many times it was
  scanned, and its short code.
- **Download PDF**, **Copy verify link** and **Reissue**.
- **Revoke**: anyone scanning the QR is then told the document is no longer valid. This cannot be undone.

When **Digitally seal documents sent out** is on, every approved quotation, invoice, order or proposal with a
verification QR goes out with the company's digital seal, so a PDF reader flags any later edit.

### What a person scanning the QR sees

The `/v/<token>` page shows one of these results:

| Result | Meaning |
|---|---|
| Authentic document | Issued and approved by your company, and still current. |
| Replaced by a newer version | Genuine, but superseded. |
| Verification expired | Genuine, but the verification period has ended. |
| No longer matches the approved version | The document changed after this copy was approved. |
| Revoked by the issuer | Withdrawn. |
| Document cancelled | Cancelled by the issuer. |
| Code not recognised | The QR does not match any document you issued. |

Depending on the policy, the page also shows the **Approval record**, the **Digital seal**, the **Electronic
signature**, and the **Issued document** with a **Download** button to compare against the copy in hand. A policy
can require sign-in: confidential documents (offer letters, by default) then ask the scanner to **Sign in** with a
company account.

![Verification page](images/approvals-verify-page.png)

---

## Send an approved document

Quotations, proposals, invoices, purchase orders and work orders are sent from Granary, never before approval.

1. Click **Send to client** (or **Send to vendor**). If the document is not approved yet, it is submitted instead
   and you are told it waits for approval.
2. Pick **Email** or **WhatsApp**.
3. For email: pick the **From** inbox if there are several (or keep the deal's inbox), fill in **To**, **Cc**,
   **Bcc** (up to 10 addresses) and the **Greeting name**. For WhatsApp: enter a **WhatsApp number or chat**, or
   pick one under **Or pick a chat** (WhatsApp needs a connected bridge).
4. Add a **Message (optional)**.
5. Click **Review**, check the summary, then **Confirm & send**.

The approved PDF with its verification QR is attached. An emailed document is recorded as a conversation in the
[CRM inbox](CRM-Inboxes) and copied to the mailbox's Sent folder, so the customer's reply lands in the same
thread. If the same document went to the same recipient in the last 15 minutes, Granary asks first and offers
**Send again**. Invoices can carry a billing email that is used before the CRM contact's.

---

## E-signature for contracts and offer letters

For most documents, the approval stamp is the company's signature. Contracts and offer letters also collect the
other party's signature. Each policy sets an **E-signature** flow:

| Flow | What happens |
|---|---|
| No e-signature (approval + QR) | The approval and the QR are the signature. |
| Counterparty signs | Your approval and QR are your side; the counterparty is invited to sign before a deadline. |
| Our signatory signs | Your designated signatory signs the approved document electronically. |
| Our signatory, then the counterparty | Your signatory signs first, then the counterparty is invited. |

The shipped contract and offer letter policies use **Counterparty signs** with a 5-business-day deadline.

To start it, open the approved document's Approval panel (for a contract, the approved request in **Approvals**)
and, in **E-signature**, click **Send for e-signature**. The section then shows the state (**Out for
signature**, **Signed**, **Declined** or **Cancelled**), who signed or declined, **Extend by** to move the deadline
(up to the policy's maximum extension) and **Open in Signing** for the full record.

---

## The Signing module

Use **Signing > Documents** to send any PDF for signature, not just approved Granary documents. You see the
documents you created (the Owner sees all). The saved views are **All**, **Drafts**, **Pending**, **Completed**,
**Declined** and **Cancelled**.

### Send a PDF for signing

1. Click **New document**. Enter the **Document title**, an optional **Message to signers**, and drop the PDF
   (PDF only). The document is uploaded.
2. **Signers**: add who needs to sign, in order.
   - **Search** picks a person from Granary (an employee, partner or vendor).
   - Or type a **Full name** and **Email**, set the **Signing order** (1 signs first) and the **Role** (**Signer**
     or **CC (observer)**), then click **Add external signer**.
   - **Self sign** adds you as a signer.
3. **Fields**: click **Open field placement editor**. Pick a field type (**Signature**, **Initials**, **Text
   Field** or **Image / Seal**), choose who it is assigned to and click on the PDF to place it. Drag to move it and
   resize from the corner.
4. **Invite**: check the document, signers, fields and signing order, then click **Send for signing**. Each signer
   gets an email with a personal link.
5. **Track**: follow each signer's progress. You can **Remind** a signer, **Sign now** if you are a signer,
   or **Cancel** the request. When everyone has signed, download the **Signed PDF** and the **Certificate**.

The completed PDF is sealed with the platform certificate. If it says "Not sealed yet", use **Regenerate signed
PDF and certificate**. To check a file's integrity, compare its SHA-256 hash (`sha256sum <file.pdf>`) with the one
shown. **Download all completed documents (ZIP)** exports every completed document.

![Signing a document](images/approvals-signing-send.gif)

### What the signer sees

The signer opens their link (`/sign/<token>`); no Granary account is needed. The link is their credential, so it
is only sent to them.

1. The PDF is shown with the fields they must complete (**Sign here**, **Initials**, **Enter text**, **Date**,
   **Check**). **Fields to complete** lists them.
2. Under **Your signature**, they pick **How to sign**: their saved signature, **Draw your signature**, or **Type
   your name**. Initials work the same way.
3. They read the **Electronic records and signature disclosure** and tick that they consent to electronic records
   and signatures and intend to sign.
4. They submit. Or they choose **Decline to sign** and give a **Reason**; the sender is notified.

Afterwards the page says **Document signed**, and whether all parties have signed or others are still to sign.

### Verify a signed document

The signing record of a document is public at `/verify/<document_id>`. It shows whether the document is **fully
executed**, the **Document details** (title, ID, signature level, digital seal, certificate, e-records consent,
the number of audit events) and the **Signatories**.

---

## Set approval policies

This is for administrators with `approvals:manage_policies`.

1. Open **Settings > Approvals & signing > Approval policies**. There is one policy per document type.
2. Select a policy. Set **Active**, **Name**, **Description**, **Default SLA (hours)** and **WhatsApp approvers
   above (base currency)** (requests above this amount also reach the approver on WhatsApp; empty means never).
3. Edit the **Approval chain**. **Add step**, reorder with **Move up** / **Move down**, or remove. For each step:
   - **Approver**: **Owner**, **Holder of a role** (pick the **Role** and, optionally, the **Department**),
     **Submitter's line manager**, **Person named on the document** (pick the **Document field**) or **A specific
     user**.
   - **SLA (hours)**, or leave it empty for the policy default.
   - **Required**: **always**, **when ANY of these apply**, **when ALL of these apply**, or **advanced (JSON)**.
     Add rules such as an amount threshold or a flag.
   - **Inform only (no decision)**: the person is told, but does not decide.
   - **Approver may approve their own submission**.
   A policy with no steps approves every submission automatically.
4. **Submission requirements**: rules a document must pass before it can be submitted, each with the **Message
   shown to the submitter**.
5. **E-signature and QR verification**: the **E-signature** flow, the **Signatory role**, the **Counterparty
   deadline (business days)**, the **Max extension (business days)**, **Print a verification QR**, **Scanner can
   view the document**, **Scanner can download**, **Require login to verify** and **Code valid for (days)**.
6. Save.

Changes apply to new submissions; requests already in progress keep their chain.

**Set the amount limits for your currency.** Limits are amounts in your functional currency. The shipped values
are sized for a currency like JPY. Change them before the first document is submitted.

### Discount allowances

Under **Discount allowances**, add rules that let a quotation discount pass without the Owner: the discount
**Type** (volume, bundle, loyalty, multi-year SaaS), a **Label**, **Min qty**, **Max qty** and **Max %**. Tick
**Inform Owner** to notify the Owner without blocking. With no allowances, every discount needs the Owner's
approval.

---

## Settings that affect this module

Under **Settings > Approvals & signing**:

- **Approval policies**: the policies and discount allowances described above.
- **E-signing**:
  - **Largest signature image** (`sign.signature_max_bytes`): the size limit for a signature or initials image.
  - **Remind signers when the deadline is within** (`sign.reminder_window_hours`) and **Between reminders to the
    same signer** (`sign.reminder_interval_hours`).
  - **Digitally seal documents sent out** (`documents.seal.outbound`): seal every approved outbound PDF with a
    verification QR. Off: the unsealed copy is sent.

Under **Settings > Company & branding > Company identity**: **Authorized signatory** and **Signatory title**,
printed under the signature.

Under **Settings > Email & WhatsApp**: the **CRM inboxes** documents are emailed from, and the **System email**
that sends approval and signing emails.

---

## Permissions

| Permission | Allows |
|---|---|
| `approvals:view` | See the Approvals inbox and the approval trail on documents |
| `approvals:manage_policies` | Edit approval policies, steps, thresholds and discount allowances; see every request |
| `verification:issue` | Issue or reissue the verified (QR) PDF of an approved document |
| `verification:revoke` | Revoke a document's QR |
| `verification:view_log` | See who scanned a document's QR and when |
| `sign:view` | See signing requests and their audit trail |
| `sign:manage` | Create signing requests, upload documents, manage signers, start and extend e-signature |
| `sign:send` | Send documents for signing (also allows everything `sign:manage` does) |
| `sales:send_quotation` | Send quotations and proposals |
| `finance:send_invoice` | Send invoices |

Who approves a step is decided by the policy, not by a permission. The Owner (superadmin) passes every check.

---

## Related pages

- [Sales](Sales): quotations, proposals and contracts
- [Finance](Finance): invoice approval
- [HR and Payroll](HR-and-Payroll): offer letters and payroll runs
- [Procurement](Procurement): purchase orders, work orders and vendor invoices
- [Knowledge and Documents](Knowledge-and-Documents): SOP and library approvals
- [CRM Inboxes](CRM-Inboxes): where sent documents appear
- [Configuration](Configuration)

<!-- nav:bottom -->
---

← Previous: [Partners](Partners) | Next: [Finance](Finance) →
<!-- /nav:bottom -->
