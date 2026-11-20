# AI Across Granary

<!-- nav:top -->
[Home](Home) › AI in Granary › **AI Across Granary**
<!-- /nav:top -->

Granary is built so that its AI, **Nary**, works inside the business processes, not beside them. Nary reads the
same records you do (leads, conversations, quotations, invoices, projects, SOPs), drafts the next step where the
work happens (a reply in the inbox, a quotation from a chat, a proposal from a deal's requirements, a project from
a won deal) and hands it to a person as a **card** to apply, edit or reject. This page is the full map: what the
AI does in each module, what starts it, and who approves the result. For how to use the chat and the approval
screens, see [Nary AI Assistant](Nary-AI-Assistant).

![Nary > Approvals with a quotation card waiting for Apply](images/ai-across-granary-approvals.png)

---

## How the AI works in Granary

There are three ways AI work starts:

1. **You click a button on a page.** For example **Score with AI** on a lead, **Quick enrich** on an organization,
   **Draft a reply with AI** in an inbox, **Ask AI about this document** on a PDF attachment, or **Ask Nary to change** on a proposal.
   Nothing runs until you click.
2. **You ask Nary in the chat.** Press **Ctrl J** (Cmd J on a Mac) on any page. When you are on a deal, lead,
   contact, organization, project, invoice or quotation page, Nary knows which record you mean. Nary answers with
   its tools, and anything that would change data comes back as a card.
3. **Nary works on its own (the Owner switches this on).** Nary reacts to business events: a new WhatsApp chat
   or email, a web form, a quotation sent, a customer accepting, a contract signed, a payment recorded, a deal won
   or lost. It prepares the next step as a card in **Nary > Approvals** for the person who holds the Owner SOP
   role. Shadow mode is on by default, so customer-facing drafts are kept and nothing is sent until you turn it off.

![Opening a Nary card, editing it and clicking Apply edited](images/ai-across-granary-card-apply.gif)

You can also connect your own AI client (any client that speaks MCP) and let it work with Granary through the
same tools and the same cards. See [MCP for AI Clients](MCP-for-AI-Clients).

---

## The map

"Card" means the result waits for a person: **Apply** / **Create** (chat cards) or **Apply**, **Edit**,
**Reject** (Nary's approval cards). "Written directly" means the click itself is the approval: the result is saved
and marked as AI work.

| Module | What the AI does | Trigger | Approval |
|---|---|---|---|
| [CRM](CRM) | Scores leads hot, warm or cold with reasons and a next step | **Score with AI** on a lead | Written directly (the score) |
| [CRM](CRM) | Writes a record brief: where the relationship stands, next best action, signals, risks | **Generate** / **Refresh** on the **AI brief** card | Written directly (read-only summary) |
| [CRM](CRM) | Fills AI attributes (summary or classify) on one record or a selection | **Recompute with AI**, or **Fill with AI** on a list selection | Written directly, marked **Filled by AI** with reasoning |
| [CRM](CRM) | Fills a company's empty fields from the web, with sources | **Quick enrich**; also when a new organization is created (`crm.enrich.on_create`) | Written directly, empty fields only; low-confidence facts are only suggested |
| [CRM](CRM) | Deep web research on a company or person: facts, people, news | **Deep research**, **Research this contact**, or Nary's **start_research** card in chat | People found wait for **Add as contact** or **Not relevant** |
| [CRM](CRM) | Creates, updates, merges and deletes records, promotes leads, sets follow-up reminders, books meetings | Chat request | Card |
| [CRM](CRM) | BANT score, requirement fit check, negotiation hints, call notes, handoff package, loss analysis (deal panel **Nary's notes**) | Nary on its own, from the conversation | Internal notes only; actions they lead to are cards |
| [CRM](CRM) | Finds duplicate organizations and deals | Chat request | Merges are cards |
| [CRM Inboxes](CRM-Inboxes) | Drafts a reply to an email or WhatsApp thread | **Draft a reply with AI** | You edit and send it yourself |
| [CRM Inboxes](CRM-Inboxes) | Reads PDF and image attachments, answers questions about them | **Ask AI about this document** on an attachment | Read only |
| [CRM Inboxes](CRM-Inboxes) | Sorts new conversations (triage), links them to contacts, makes leads from web forms and new chats, describes photos and PDFs, transcribes voice notes on your server | Nary on its own, on each new message | Leads from web forms and new prospects are created automatically (also while Nary is paused); promoting them, links, held chats and sends are cards |
| [CRM Inboxes](CRM-Inboxes) | Replies to customers (answer, price answer, qualifying questions, follow-ups), alerts you when a customer answers | Nary on its own, or a chat request | Card (shadow drafts while shadow mode is on) |
| [Sales](Sales) | Drafts quotations from a conversation or a chat request, priced by the pricing service | Chat request, or a customer asking for a quote | Card, then the normal quotation approval |
| [Sales](Sales) | Updates, reprices, withdraws, cancels, revises a quotation, links it to a deal | Chat request | Card |
| [Sales](Sales) | Requirement discovery and Solution Proposal drafts with charts, diagrams and an internal cost sheet | Nary on its own (complex deals) or a chat request | Card, then the normal proposal approval |
| [Sales](Sales) | Rewrites proposal sections and figures | **Ask Nary to change** on the proposal | Shown in the editor for you to keep editing and save |
| [Sales](Sales) | Follow-up cadence after a quotation is sent | Nary on its own | Card per follow-up |
| [Sales](Sales) | Contract draft when the customer accepts | Nary on its own, or a chat request | Card, then the contract's own approval |
| [Sales](Sales) | Researches current market rates for the rate card | **Ask Nary to research current rates** | Card per rate change |
| [Sales](Sales) | Catalog changes (prices, products, tiers, product files) | Owner's chat request | Card (Owner only) |
| [Approvals and Signing](Approvals-and-Signing) | Nary's quotations, proposals, contracts and invoices enter the same approval policies; it sends only approved, QR-verified PDFs | Any Nary draft or send | Approval policy, then a send card |
| [Finance](Finance) | Lists overdue invoices, revenue summary; drafts proforma and down-payment invoices | Chat request; Nary on its own after a contract draft | Read only; invoices are cards |
| [Finance](Finance) | Moves a deal to Won when the contract is signed and the down payment is recorded | A payment recorded or a contract signed | Automatic rule (Nary must be running) |
| [Multi-Currency](Multi-Currency) | Prices quotation drafts and price answers in the customer's currency | With every quotation draft | Same cards as Sales |
| [Projects](Projects) | Reads projects; starts the project of a won deal with its PM and kickoff tasks | Chat request; Nary on its own on Won | Kickoff is a card |
| [R&D](RnD) | Files ideas for tools it is missing in the R&D backlog | Chat request it cannot do | Filed directly as a backlog item |
| [Knowledge and Documents](Knowledge-and-Documents) | Answers from SOPs with citations; reads and sends library documents; drafts SOPs; converts PDFs to articles; drafts SOP revisions | Chat; **Source PDF** on a new article; **Nary > Report** | SOP drafts and sends are cards; articles are published by people |
| [Collaboration](Collaboration) | Searches and reads your own mailbox; chats with staff on WhatsApp | Chat request; a message from your linked WhatsApp chat | Read only; cards come back as a numbered list on WhatsApp |
| [Partners](Partners) | Routes new leads to a partner by territory or rule; warns before protection ends; gathers evidence for conflicting registrations | Nary on its own, on a new lead | Routing is a card |

Modules where Nary has no tools yet: [Marketing](Marketing), [Procurement](Procurement),
[Manufacturing and Inventory](Manufacturing-and-Inventory), [Operations](Operations),
[HR and Payroll](HR-and-Payroll) and [Face Clock-In](Face-Clock-In). Nary can explain how these modules work
from its built-in ERP guide, but it can't read or change their records. Face matching in Face Clock-In uses its
own face service, not Nary.

---

## Principles

### Nary has your permissions, never more

Every tool Nary uses checks the permissions of the person it works for. A staff member who can only see their
own deals gets answers about their own deals. When you apply a card, the server checks your permissions again at
that moment. A connected AI client acts as the person who connected it, narrowed further by its access level.

When Nary talks to a **customer**, it gets a much smaller set: that customer's own leads, deals and quotations,
the catalog at list prices, documents marked customer-safe, and approved company facts. Costs, volume tiers,
margins, other customers and internal notes never reach that context.

### Cards and autonomy

Every action Nary can take (send a reply, raise an invoice, merge two contacts, change the catalog) has an
**autonomy level** in **Nary > Autonomy**: **Shadow** (draft only), **Ask** (a card), **Act and tell** or
**Silent**. Out of the box, almost every action is **Ask**. Approving cards without meaningful edits builds a
streak, and the Owner can then promote an action. Anything involving money, quotations, contracts, merges,
deletes and catalog changes is on the never-promote list and always asks.

Beyond the levels, there are company-wide switches:

- **Shadow mode** (on by default): customer-facing actions are kept as drafts and compared with what you send.
- **Pause**: Nary does nothing on its own; briefs still arrive and the **Work queue** lists what waits for you
  and your AI client.

Every piece of work Nary does is a **run** you can inspect in **Nary > Runs**: trigger, model, steps and cost.

### AI-only knowledge

**Settings > Nary & AI > AI knowledge** is a library that only the AI reads: pricing and discount rules,
product facts, playbooks, how you work. Staff can't browse it. Nary uses it in the chat, reply drafts, quotation
drafts, scoring, briefs and connected AI clients. Only documents you mark **Customer-safe** are used when Nary
writes to a customer. A built-in **ERP guide**, generated from Granary's code and settings, tells Nary how each
module works.

### Any model provider

Granary does not depend on one AI vendor. In **Settings > Nary & AI > Providers & keys** you add keys for:

| Provider | Notes |
|---|---|
| Anthropic | Chat and tools; no embeddings API |
| OpenAI, or any OpenAI-compatible endpoint | Set a base URL to use another compatible service |
| DeepSeek | OpenAI-compatible |
| Gemini | Chat, embeddings, and grounded web search |
| Ollama | Self-hosted local models, no key; Granary talks to its OpenAI-compatible API |

In **Model per job** you pick the provider and model for each job (chat, scoring, enrichment, research,
embeddings, each Nary task), so you can run cheap jobs on a small or local model and keep a larger one for
drafting. Embeddings need a provider with an embeddings API (OpenAI-compatible, Gemini or Ollama). Web research
uses a self-hosted SearXNG, the provider's own grounded search, or a search API (Tavily or Brave).

Some jobs run a cheap model first and let code (not the model) decide when to ask a stronger one, for example
when a "not business" verdict contains business signals. Voice notes are transcribed on your own server.

### Cost control

A monthly budget in USD with warn, stop-non-essential and stop-all thresholds applies to every AI call. Usage is
shown in **Settings > Nary & AI > Budget**. Features such as lead scoring, briefs and research never run on their
own except where a setting says so (for example enrichment on create).

### House style

Everything the AI writes follows the same house style, including the **No em dashes in anything the AI writes**
switch. Nary's customer messages use the style guide the Owner approved, and procedures Nary learns (skills)
wait for the Owner's approval in **Nary > Memory** before Nary uses them.

---

## Settings that affect AI across modules

| Setting | What it does |
|---|---|
| **AI features** (master switch) | Off stops every AI feature |
| **Nary chat**, **Draft quotations and invoices**, **CRM lead scoring**, **Reply and draft suggestions**, **Document intelligence** | Switch each family of features on or off |
| `ai.routing` (**Model per job**) | Provider and model per job |
| `ai.budget.monthly_usd` and thresholds | The monthly AI budget |
| `nary.paused`, `nary.shadow_mode` | Nary's own work, and whether customer-facing actions are only drafts |
| `nary.autonomy.levels`, `nary.autonomy.never_promote` | How far each action is trusted |
| `crm.enrich.on_create` | Enrich new organizations automatically |

## Permissions

- Any staff account: chat with Nary and apply your own chat cards (checked against your permissions).
- `nary:view`, `nary:approve`, `nary:manage`: see Nary's pages, decide its cards, change how it works.
- Owner or system admin: **Settings > Nary & AI**, including keys, routing, budget and AI knowledge.
- Each module's own permissions decide what Nary may read or change there for you.

## Related pages

- [Nary AI Assistant](Nary-AI-Assistant)
- [MCP for AI Clients](MCP-for-AI-Clients)
- [Configuration](Configuration)

<!-- nav:bottom -->
---

← Previous: [FAQ](FAQ) | Next: [Nary AI Assistant](Nary-AI-Assistant) →
<!-- /nav:bottom -->
