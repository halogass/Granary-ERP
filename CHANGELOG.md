# Changelog

All notable changes to Granary are listed here. Granary follows [Semantic Versioning](https://semver.org/): from
1.0.0 on, every upgrade keeps your data (database changes always ship as migrations).

## 1.0.1 (2026-11-21)

### Fixed
- Messaging: thread panels, conversation previews, the dashboard widget and notifications showed raw `<#deal:...>`
  and `<@user:...>` tokens; they now show the same chips (or readable labels) as the channel view, and thread replies
  show their attachments.
- Nary approvals: editing a WhatsApp reply, email reply or customer message opens a plain message box (and the
  subject for email) instead of a raw JSON editor; after "Apply edited" the card shows the edited text; the Waiting
  count and the menu badge update at once.

## 1.0.0 (2026-11-21)

The first public release.

### The AI-native ERP
- **Nary**, the built-in AI assistant, works inside the business processes: it scores and researches leads, writes
  AI briefs of records, enriches new companies, triages email and WhatsApp, drafts replies, quotations, proposals,
  contracts and invoices, follows up on quotations, hands won deals to delivery, and answers from your own SOPs
  with citations.
- **People stay in control**: every action that commits the company arrives as a card a person applies, edits or
  rejects; autonomy can be raised per kind of action.
- **Bring your own AI**: Anthropic, OpenAI or any OpenAI-compatible API, Gemini, DeepSeek, or local models through
  Ollama, chosen per job, with a monthly budget.
- **MCP server**: connect your own AI client; it signs in as a person and gets exactly that person's permissions.

### Modules
CRM and shared inboxes, sales (quotations, proposals, contracts, catalog and price lists), approvals with verified
PDFs and e-signature, finance (invoices, payments, bank reconciliation, ledger, taxes, assets, budgets) with
multi-currency, procurement with a vendor portal, manufacturing and inventory, operations (fleet, maintenance,
helpdesk), HR and payroll, face clock-in with liveness checks, projects, R&D with GitLab issue sync, knowledge base
and document library, messaging and video calls, partners with a partner portal, marketing and email campaigns.

### Platform
- Rust backend (axum, sqlx, tokio), Next.js frontend, PostgreSQL with pgvector, Redis, SeaweedFS (S3) storage.
- One-command self-hosting with Docker Compose.
- Every business number (taxes, payroll rules, approval limits, schedules) is a setting, not code.
- Security: two-factor sign-in, session families, password plus authenticator for powerful AI access, verified
  documents, rate-limited public forms, refusal to start with placeholder secrets.

### Documentation
- [Wiki](https://github.com/halogass/Granary-ERP/wiki): installation, first steps, a guide to every module, AI across
  Granary, administration and a developer guide.
