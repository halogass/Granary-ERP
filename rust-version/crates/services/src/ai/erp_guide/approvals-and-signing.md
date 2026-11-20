# Approvals, QR verification, sending to clients and e-sign

One approval engine decides every document that needs a yes; an approved document carries a QR code anyone can check, can be sent to the client from the ERP, and can be signed electronically.

**The engine** (/approvals, policies in Settings → Approvals & signing → Approval policies)
- Submit → route → approve or reject. The chain is fixed at submit; one pending request per document; rejecting needs a comment and makes the document editable again.
- An approval is bound to the document's facts: any edit after approval makes it stale and sending is refused until it is approved again.
- The send/activate gate: approved and unchanged → go; otherwise it submits (the Owner submitting approves at once) and answers "approval pending".
- Approver modes: the Owner (a superadmin whose position starts with Owner), a role (optionally in a department), the submitter's manager, a named user. The submitter can't approve their own request unless the step allows it (superadmins can).
- Default policies: quotation (Owner unless all at list price), proposal (Owner), invoice (Owner), contract (Owner, 48h SLA), purchase order (team lead up to the policy limit, Owner above; 2 vendor quotes above a second limit), contractor request (Owner), work order (Owner above the request's estimate), vendor invoice (Finance Admin up to 25M, Owner above), expense (manager, Finance Admin, Owner when large), marketing content, hiring request, offer letter, SOP article, payroll run (Owner).
- Hourly SLA reminders. Seeing requests needs approvals:view; editing policies needs approvals:manage_policies.

**Approval as signature and QR**
- A policy's signature flow: none, counterparty, internal, or internal then counterparty. With counterparty, the company's approval is its signature.
- Approved documents with QR enabled carry a QR on every page pointing to a public check page that re-validates on every scan (valid, revoked, superseded, expired, void, not approved). HR documents need a login. Issuing and revoking need verification:issue / verification:revoke.

**Sending to the client**: quotations, invoices, purchase orders, work orders and proposals are sent from the ERP by email or WhatsApp (each with its send permission), through the approval gate. The same document to the same person within 15 minutes needs confirming again.

**E-sign** (/sign/documents)
- A sign document is draft, pending, completed, declined, expired or cancelled; signers sign in order (pending, sent, viewed, signed, declined); completion produces a certificate.
- From an approved document, the ERP issues the verified PDF, places signature fields and invites the signers; the counterparty deadline is in business days and can be extended up to the policy's limit.
- When signing completes: a contract becomes signed; an offer letter becomes accepted (the applicant to offer_accepted); a quotation records its signing date (its status changes only by recording the PO).
- Permissions: sign:view, sign:send, sign:manage.
<!-- added after the Python freeze (Nov 2026) -->
- **Digital seal on every document sent** (3 Nov 2026): every approved document with a verification QR (quotations, invoices, purchase and work orders, proposals) leaves the ERP with the company's digital seal, a PAdES certification signature: the same sealed PDF goes by email and WhatsApp (also as Nary or MCP client attachments), and is what the public check page (/v/...) shows and downloads and what staff download. A PDF reader (Acrobat) shows it in its signature panel as certified by the company and flags any edit made after sealing; the text stays selectable and nothing is encrypted. The seal allows form filling and further signatures (DocMDP level 2), so a customer can still add their own digital signature. While the company uses its own self-signed certificate, Acrobat shows a yellow "validity unknown" banner (the seal is intact, its signer is not on Adobe's trust list); a certificate from a trusted certificate authority (Adobe-trusted) installed on the server is used automatically and removes it. The sealed copy is made at approval time; documents issued earlier are sealed on their next send or download while still valid. If sealing fails the unsealed copy goes out (its QR still verifies it) and the Owner gets one alert for that document. The check page says "Digitally sealed by" and the company name with the seal level. documents.seal.outbound (Settings → Approvals & signing → E-signing) turns it off. The e-sign flow still signs the unsealed original and seals the signed result as before.
