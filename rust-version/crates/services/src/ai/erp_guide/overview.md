# Overview: how Granary fits together

Granary is an open-source ERP: CRM and sales, finance, projects, HR, manufacturing, procurement, partners, operations, knowledge and Nary (the AI assistant) in one system, with one approval engine and one permission model.

- **Money flow**: lead → deal → (proposal) → quotation → Owner approval when not at list price → sent to the customer → PO recorded (signed) → proforma or commercial invoice (Owner approves every invoice) → payment → general ledger. A Won deal is handed over to a project.
- **Approvals**: one engine for every document that needs a decision (quotations, invoices, contracts, POs, expenses, payroll, offer letters, SOPs...). Sending or activating a document goes through its gate; an edit after approval makes the approval stale. See approvals-and-signing.
- **Who may do what**: permission codes `module:action` grouped in roles (see permissions). Nary's tools act with the permissions of the person they act for.
- **Business numbers** (tax rates, social contributions, thresholds, reminders, prices rules) are settings, never fixed in code; a missing one stops the feature with a clear error naming the key (see settings).
- **Sections of this guide**: crm, sales, finance, approvals-and-signing, projects, hr, manufacturing, partners-and-vendors, operations, marketing, knowledge-and-ai, nary, settings-and-roles; generated: crm-picklists, crm-fields, permissions, settings, api.
