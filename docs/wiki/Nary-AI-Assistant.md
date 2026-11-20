# Nary AI Assistant

<!-- nav:top -->
[Home](Home) › AI in Granary › **Nary AI Assistant**

**On this page:** [Open Nary](#open-nary) · [Ask Nary](#ask-nary) · [Confirm cards](#confirm-cards) · [Nary as an assistant that works on its own](#nary-as-an-assistant-that-works-on-its-own) · [AI knowledge](#ai-knowledge) · [Set up the AI provider](#set-up-the-ai-provider) · [Settings that affect Nary](#settings-that-affect-nary) · [Permissions](#permissions)
<!-- /nav:top -->

**Nary** is Granary's built-in AI assistant. Every staff member can chat with it to look things up (SOPs,
customers, deals, invoices, projects, email history), get summaries and drafts, and prepare changes. Nary works
with **your** permissions: it can't show you anything you couldn't open yourself. Anything that would change
data comes back as a **card** that a person applies or discards; nothing is saved until then. Beyond the chat,
Nary can work as the company's sales and operations assistant: it takes in new leads, drafts replies and
follow-ups and queues them for the Owner's approval. The Owner and system admins set it up.

![Nary's chat on the full page](images/nary-chat.png)

---

## Open Nary

- **Chat panel:** click **Ask AI** in the top bar, or press **Ctrl J** (Cmd J on a Mac), on any page. The panel
  stays beside the page you are working on. Its buttons: **Open full page**, **History**, **New chat**, **Close**.
- **Full page:** open **Nary** in the sidebar, then the **Chat** tab (`/nary/chat`). Your earlier chats are listed
  under **Your chats**.

Everyone with a staff account can use the chat. Portal users (vendors and partners) can't.

---

## Ask Nary

1. Type a question and press Enter. Examples:
   - "What does our SOP say about approving a quotation discount?"
   - "Summarise my open deals and which ones look stalled."
   - "Which invoices are overdue right now?"
   - "What are my tasks due this week?"
2. Nary answers as it goes (the reply streams in).
3. To give it a file, use the attach button: PDF, Word, Excel, a photo, or (where offered) meeting notes or a
   recording.

What Nary uses to answer:

- the closest **SOPs** from the knowledge base and matching **AI knowledge** documents (see below);
- **tools** that run as you and check your permissions: SOP and knowledge search, CRM records, the pipeline, deals
  and contacts, invoices, projects, email and chat history, and, when web research is set up, web search and page
  reading.

Each question can take several tool steps. If Nary runs out of steps, it still gives you an answer with what it
has.

---

## Confirm cards

When you ask for a change (a quotation draft, an invoice draft, an SOP draft, a CRM change, a follow-up reminder,
a customer message, a meeting), Nary does not save it. It shows a **card** with exactly what would change.

![A confirm card in the chat](images/nary-confirm-card.png)

- Click **Apply** (or **Create**) to save it. The server checks your permissions again at that moment.
- Click **Discard** to drop it.

![Asking Nary for a follow-up reminder and applying the card](images/nary-confirm-flow.gif)

The outcome is stored on the message and given to Nary on your next turn, so it knows what you decided. After a
page reload, the card shows whether it was applied or discarded.

---

## Nary as an assistant that works on its own

The other tabs under **Nary** are for the people who run Nary (permission `nary:view` to look, `nary:approve` to
decide cards, `nary:manage` to change how it works). Staff without `nary:view` see only the chat.

| Tab | What it shows |
|---|---|
| **Overview** | Cards waiting for you, shadow drafts, runs and failed runs in the last 24 hours, the **Switches** (pause, shadow mode), rhythm and budget, and the **SOP roles** (who holds each role; Nary's cards go to whoever holds Owner) |
| **Approvals** | What Nary wants to do. Open a card and **Apply**, **Edit** or **Reject** it; each decision teaches Nary |
| **Intake** | Every new conversation, sorted: what Nary filtered out, who it linked to whom, and chats it won't read until you say so |
| **WhatsApp history** | A one-time import of old WhatsApp chats (needs a WhatsApp bridge) |
| **Meetings** | Customer calls: proposed times, bookings and what came out of each call |
| **Report** | The weekly report |
| **Runs** | Everything Nary did: trigger, SOP, model, steps and cost |
| **Memory** | What Nary knows and where it learnt it. You can add company facts. Anything from a customer is information, never an instruction |
| **Autonomy** | How far each kind of action is trusted (see below) |
| **Work queue** | What waits for you (and your AI client) while Nary is paused |

### Approvals of Nary's actions

Nary's own actions (a reply to a customer, a follow-up, a quotation change) go to **Nary > Approvals** as cards
for the person who holds the Owner SOP role. On a card:

- **Apply** sends or saves it as written;
- **Edit** lets you change it, then **Apply edited**;
- **Reject** drops it, with an optional **Reason for rejecting**.

Cards that are scheduled show **Send now** and **Drop** instead. Undecided cards expire after
`nary.pending.expire_days`.

### Autonomy

Each kind of action has a level:

| Level | Meaning |
|---|---|
| **Shadow** | Nary drafts but nothing is sent; drafts are compared with what you actually send |
| **Ask** | Every action waits for your approval |
| **Act and tell** | Nary acts and tells you |
| **Silent** | Nary acts without telling you |

Approving cards without meaningful edits builds a streak. After `nary.autonomy.promotion_count` approvals in a row,
an action can be promoted by the Owner. A meaningful edit or a reject moves it down one level (not below
`nary.autonomy.demotion_floor`). Actions on the never-promote list always ask.

### Pause and shadow mode

On **Nary > Overview**, **Pause** stops Nary's runs and actions; events wait, briefs still arrive, and the
**Work queue** collects what needs doing. **Resume** starts it again. **Shadow mode** keeps customer-facing drafts
without sending them. Both need `nary:manage`.

---

## AI knowledge

**AI knowledge** is a library of documents only the AI reads: pricing and discount rules, product facts,
playbooks, how you work. Employees can't browse it, and it is separate from the knowledge base. The AI uses it in
the chat, reply suggestions, drafting, lead scoring, record briefs and connected AI clients, and may quote it.

Open **Settings > Nary & AI > AI knowledge** (Owner and system admins):

1. Drop Markdown (`.md`, up to 5 MB) or PDF (up to 25 MB) files, or use **Upload files**. Or click **Write Markdown** to
   type it in.
2. Optional: **When it applies**, a note on when the document is relevant.
3. Mark a document **Customer-safe** only if Nary may use it when talking to a customer. Leave it off for costs,
   tiers, margins or internal rules.
4. Switch a document off to keep it without the AI using it.
5. Use **Test: what would the AI find?** to check which passages a question matches.

After changing the embedding model, use **Re-index**. One built-in document, the ERP guide, is generated from
Granary's code and settings and kept up to date automatically; it can be switched off, not edited.

---

## Set up the AI provider

Everything AI is configured in **Settings > Nary & AI**, by the Owner and system admins.

1. **AI features**: the master switch (off stops every AI feature, Nary included), **No em dashes in anything
   the AI writes**, and switches for **Nary chat**, **Draft quotations and invoices**, **CRM lead scoring**,
   **Reply and draft suggestions** and **Document intelligence**.
2. **Providers & keys**: paste the key for each provider you use (Anthropic, OpenAI or an OpenAI-compatible
   endpoint, Gemini, DeepSeek; Ollama is self-hosted and needs no key). Click **Save**, then **Test**. Keys are
   **stored encrypted** in the database and never shown again. They are never put in `.env`.
3. **Model per job**: the provider and model for each job (chat, enrichment, research, embeddings, web search,
   each Nary task). The shipped routing uses Anthropic for most jobs and Gemini for embeddings and web search, so
   **change it to the providers you have keys for**. Embeddings need a provider with an embeddings API.
4. **Budget**: the monthly budget in USD (default 100, 0 = no cap), when to warn, when to stop non-essential jobs
   and when to stop everything. This month's usage is shown here.
5. **Chat persona** and **AI chat**: how Nary writes and what it knows about the ERP and catalog.
6. **Web research**: **SearXNG (self-hosted, free)** or another search backend. CRM enrichment and research need
   one.

![Settings > Nary & AI > Providers & keys](images/nary-provider-keys.png)

Provider errors reach the user in plain words (a rejected key, an unknown model, a rate limit, a timeout). See
[Troubleshooting](Troubleshooting#nary-answers-with-an-error).

---

## Settings that affect Nary

| Setting | What it does |
|---|---|
| AI features (master switch) | Off stops every AI feature |
| `ai.routing` (Model per job) | Provider and model per job |
| `ai.budget.monthly_usd`, warn/stop percentages | The monthly AI budget |
| `ai.no_em_dash` | Strip em dashes from all AI output |
| `nary.paused` | Pause Nary's own runs and actions |
| `nary.working_hours`, `nary.brief_times` | Nary's day and brief times |
| `nary.autonomy.promotion_count`, `nary.autonomy.edit_threshold_pct`, `nary.autonomy.demotion_floor` | How autonomy changes |
| `nary.pending.expire_days` | Undecided cards expire after |
| `nary.queue.*` | The work queue and its WhatsApp nudges |

## Permissions

- Any staff account: chat with Nary, apply or discard your own chat cards (each change is checked against your
  own permissions).
- `nary:view`: see Nary's pages (overview, approvals, intake, runs, memory...).
- `nary:approve`: decide Nary's cards; also opens the work queue.
- `nary:manage`: pause, shadow mode, SOP roles, autonomy and other changes to how Nary works.
- Owner or system admin: Settings > Nary & AI, including AI knowledge and provider keys.

## Related pages

- [MCP for AI Clients](MCP-for-AI-Clients)
- [Knowledge and Documents](Knowledge-and-Documents)
- [CRM](CRM)
- [Configuration](Configuration)

<!-- nav:bottom -->
---

← Previous: [AI Across Granary](AI-Across-Granary) | Next: [MCP for AI Clients](MCP-for-AI-Clients) →
<!-- /nav:bottom -->
