# Granary

<!-- nav:top -->
**On this page:** [Where to start](#where-to-start) · [Getting started](#getting-started) · [AI in Granary](#ai-in-granary) · [Using Granary](#using-granary) · [Administration](#administration) · [Developers](#developers) · [A few things worth knowing](#a-few-things-worth-knowing)
<!-- /nav:top -->

Granary is an open-source, self-hosted, AI-native ERP for small and mid-sized companies. Its AI, **Nary**, is part
of the business processes, from the first customer message to the project kickoff, not a chatbot on the side (its
chat is one keystroke away on every page). It sorts new customer conversations and turns prospects into leads,
scores and researches them, drafts replies, quotations, proposals, contracts and invoices, follows up after a
quotation, starts the project when a deal is won, and answers from your own SOPs and documents. Nary works with
each person's own permissions, and its drafts and proposed changes arrive as **cards** that a person applies, edits
or rejects. You choose the AI provider (Anthropic, OpenAI or any OpenAI-compatible service, DeepSeek, Gemini, or
local models through Ollama), and you can also connect your own AI client over MCP. See [AI Across
Granary](AI-Across-Granary) for the full map.

One web application covers CRM and sales, finance and accounting, HR and payroll, projects, manufacturing and
inventory, procurement, partners, marketing, internal messaging and video calls. The backend is written in Rust,
the frontend in Next.js, and everything runs with Docker Compose on one server. Granary is dual-licensed under MIT
OR Apache-2.0.

![The Granary dashboard after sign-in](images/home-dashboard.png)

---

## Where to start

| You are | Start here |
|---|---|
| Installing Granary on a server | [Installation](Installation), then [First Steps](First-Steps) |
| The person who set it up (the Owner) | [First Steps](First-Steps), then [Administration](Administration) and [Configuration](Configuration) |
| A staff member | Find your module in the list below |
| Curious what the AI does in each module | [AI Across Granary](AI-Across-Granary) |
| Connecting an outside AI client | [MCP for AI Clients](MCP-for-AI-Clients) |
| A developer | [Developer Guide](Developer-Guide) |
| Stuck | [Troubleshooting](Troubleshooting) and the [FAQ](FAQ) |

---

## Getting started

| Page | What it covers |
|---|---|
| [Installation](Installation) | Requirements, Docker Compose, `.env`, first start, reverse proxy, TURN |
| [First Steps](First-Steps) | What to set right after installing |
| [FAQ](FAQ) | Short practical answers |

## AI in Granary

| Page | What it covers |
|---|---|
| [AI Across Granary](AI-Across-Granary) | What the AI does in each module, what starts it and who approves it |
| [Nary AI Assistant](Nary-AI-Assistant) | The assistant, confirm cards, autonomy, AI knowledge, provider keys |
| [MCP for AI Clients](MCP-for-AI-Clients) | Connecting an outside AI client: browser login, tokens, levels |

## Using Granary

### Sell

| Page | What it covers |
|---|---|
| [CRM](CRM) | Leads, contacts, organizations, deals and the pipeline, activities, follow-ups, duplicates |
| [CRM Inboxes](CRM-Inboxes) | Shared email inboxes, scheduled sends, the WhatsApp inbox |
| [Sales](Sales) | Quotations, proposals, contracts, the product catalog |
| [Marketing](Marketing) | Campaigns, content calendar, email campaigns |
| [Partners](Partners) | Partner accounts, deal registration, commissions, the partner portal |

### Approve and sign

| Page | What it covers |
|---|---|
| [Approvals and Signing](Approvals-and-Signing) | Approval policies, verification QR codes, e-signature, sending documents |

### Money

| Page | What it covers |
|---|---|
| [Finance](Finance) | Invoices, payments, expenses, ledger, bank, taxes, reports, assets, budgets |
| [Multi-Currency](Multi-Currency) | Functional currency, exchange rates, FX gain and loss |

### Buy and make

| Page | What it covers |
|---|---|
| [Procurement](Procurement) | Vendors, RFQs, purchase orders, receipts, vendor invoices, the vendor portal |
| [Manufacturing and Inventory](Manufacturing-and-Inventory) | Products, bills of materials, production, QC, warehouses, warranty |
| [Operations](Operations) | Fleet, maintenance, helpdesk |

### People

| Page | What it covers |
|---|---|
| [HR and Payroll](HR-and-Payroll) | Employees, attendance, leave, payroll, hiring, offer letters |
| [Face Clock-In](Face-Clock-In) | Face matching, liveness, geofence, kiosk tablets |

### Work together

| Page | What it covers |
|---|---|
| [Projects](Projects) | Projects, tasks, time logs |
| [R&D](RnD) | Product lines, R&D projects, backlog, GitLab issue sync |
| [Knowledge and Documents](Knowledge-and-Documents) | Articles, SOPs, the document library |
| [Collaboration](Collaboration) | Channels and direct messages, personal email, video calls |

## Administration

| Page | What it covers |
|---|---|
| [Configuration](Configuration) | How settings work, environment variables, the settings areas |
| [Administration](Administration) | Users, roles, portal users, two-factor sign-in, sessions, security |
| [Backups and Upgrades](Backups-and-Upgrades) | Backups, restores, updating Granary, routine checks |
| [Troubleshooting](Troubleshooting) | Common problems and their fixes |

## Developers

| Page | What it covers |
|---|---|
| [Developer Guide](Developer-Guide) | Architecture, repository layout, building, tests, CI, contributing |

---

## A few things worth knowing

- **Business numbers are settings.** Tax rates, payroll contributions, approval limits, reminder schedules and
  currency decimals are edited in **Settings**, not in code. When one is missing, the action fails with a
  message that names the setting. See [Configuration](Configuration).
- **The AI asks before it acts.** Nary's drafts and proposed changes wait as cards until a person confirms them.
  The few things that run without a card (lead intake, enrichment of new organizations, buttons you click) are
  listed in [AI Across Granary](AI-Across-Granary). See also [Nary AI Assistant](Nary-AI-Assistant).
- **WhatsApp needs a bridge you provide.** Granary does not ship a WhatsApp Web bridge. Without one, the
  WhatsApp features show as disconnected and everything else works.
- **Security problems** are reported privately through the repository's **Security** tab, never in a public
  issue. See [Administration](Administration#reporting-a-security-problem).

<!-- nav:bottom -->
---

Next: [Installation](Installation) →
<!-- /nav:bottom -->
