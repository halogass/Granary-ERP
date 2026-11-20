# CRM

<!-- nav:top -->
[Home](Home) › Using Granary › Sell › **CRM**

**On this page:** [AI in this module](#ai-in-this-module) · [Organizations](#organizations) · [Contacts](#contacts) · [Leads](#leads) · [Deals and the pipeline](#deals-and-the-pipeline) · [Activities](#activities) · [Custom attributes](#custom-attributes) · [AI brief](#ai-brief) · [AI enrichment and web research](#ai-enrichment-and-web-research) · [Follow-up reminders](#follow-up-reminders) · [Duplicates and merge](#duplicates-and-merge) · [Filtered search](#filtered-search) · [Settings that affect this module](#settings-that-affect-this-module) · [Permissions](#permissions)
<!-- /nav:top -->

The CRM keeps track of the companies and people you sell to and of every sales opportunity, from the first
inquiry to a won deal. Sales staff use it every day to log leads, move deals through the pipeline, record calls
and meetings, and set follow-up reminders. Managers use the pipeline and the follow-up list to see what is
stuck. AI features (lead scoring, record briefs, AI attributes, company enrichment and web research) run only
when you click them.

The CRM is organized in four kinds of records:

| Record | What it is |
|---|---|
| Organization | A company (customer, prospect, partner). Example: Kumo Trading Co.'s customer "Hoshino Design Office". |
| Contact | A person, usually linked to an organization. |
| Lead | An early inquiry that you have not qualified yet. A qualified lead is promoted to a deal. |
| Deal | A real sales opportunity with a value, a stage and a probability. Quotations and proposals hang off a deal. |

![Leads board](images/crm-leads-board.png)

---

## AI in this module

The AI works on your CRM records in three ways: buttons on the page, the Nary chat, and Nary's own work on
customer conversations. Buttons write their result when you click them; everything else arrives as a card that a
person applies. See [AI Across Granary](AI-Across-Granary) for the full map.

![An organization after Quick enrich, with the Web research panel and the AI brief](images/crm-ai-enrichment.png)

- **Lead scoring**: click **Score with AI** on a lead. The AI reads the lead and its WhatsApp and email history
  and returns **Hot**, **Warm** or **Cold**, a score, the reasons and a next step.
- **AI brief**: click **Generate** on any record's **AI brief** card for where the relationship stands, the next
  best action, signals and risks.
- **AI attributes**: custom fields the AI fills on request (**Recompute with AI**, or **Fill with AI** on a list
  selection), each with its reasoning and sources.
- **Enrichment and research**: **Quick enrich** fills only the empty fields of an organization from the web.
  **Deep research** and **Research this contact** find facts and people; you add each person with **Add as
  contact** or drop it with **Not relevant**. New organizations can be enriched automatically
  (`crm.enrich.on_create`).
- **Ask Nary about a record**: press **Ctrl J** on a lead, deal, contact or organization page and ask, for
  example, "What happened with this deal since the demo?" or "Log the call and remind me on Friday". Nary reads
  the record's history (activities, emails, WhatsApp, meetings) and prepares changes as cards: create or update a
  record, promote a lead, merge duplicate contacts, organizations, leads or deals, set or move a follow-up
  reminder, book or cancel a meeting. Click **Apply** to save.
- **Nary's notes**: when Nary works on its own, deal and lead pages show a **Nary's notes** panel: the BANT score,
  the requirement fit check, negotiation hints, call notes, the handoff package, the follow-up cadence and
  meetings. These are internal and never reach the customer. People with `nary:approve` can click **Propose
  meeting times** (the times go to **Nary > Approvals** as a card) or add a **Debrief note** or **Debrief file**
  after a call and click **Send debrief**: Nary sets the reminders and drafts the summary email for the customer.

![Nary's notes on a deal: BANT score, fit check and follow-up cadence](images/crm-ai-deal-nary-notes.png)

---

## Organizations

Open **CRM > Organizations**. The list has the saved view tabs **All**, **Added this month**, **With email** and
**With phone**, industry chips, and **Search organizations** (it searches the name, website, email, city, country
and industry).

### Add or edit an organization

1. Click **Add organization**.
2. Fill in **Company name** and, if you know them, **Industry**, **Website**, **Email**, **Phone**, **City**,
   **Country** and **Notes**.
3. Click **Create organization**.

To change one later, expand its row and click **Edit**, change the fields and click **Save changes**.

### The organization page

Click **Open organization** on a row to open its record page. The left column shows **Details**, the
**Attributes** (custom and AI fields, see below) and **Related** (how many contacts, leads and deals it has). The
right column shows the **AI brief** and the **Timeline** of everything that happened with this company.

On this page you can also:

- Turn on **Quote without tax**. New quotations, invoices and proformas for this customer then start with no tax
  on every line. You can still add a tax to a single line. This needs `crm:edit_contact`.
- Click **Quick enrich** or **Deep research** (see [AI enrichment and web research](#ai-enrichment-and-web-research)).

### Delete organizations

Expand a row and click **Delete**, then confirm. To delete several, tick them in the list and click
**Delete N selected**, then **Delete**. Organizations that cannot be deleted are listed with the reason; the
others are removed. Deleting needs `crm:delete_contact`.

---

## Contacts

Open **CRM > Contacts**. The saved view tabs are **All**, **Added this month**, **On WhatsApp** and
**No organization**. The search box finds people by name, email and company.

### Add a contact

1. Click **Add contact**.
2. Fill in **Full name**, and optionally **Position**, **Organization**, **Email**, **Phone**, **WhatsApp** (in
   international format, for example `+81 3 5555 0100`) and **Notes**.
3. Click **Create contact**.

On the list, a WhatsApp icon opens a chat with the person in WhatsApp. **Open contact** opens the record page with
the same layout as an organization: details (position, department, email, phone, WhatsApp, LinkedIn),
attributes, attachments, AI brief and timeline. Use **Research this contact** to look up the person's role and
LinkedIn profile from public sources.

> Contacts, leads and deals have an owner. With `crm:view_own` you only see the ones assigned to you. With
> `crm:view_all` you see everyone's. Organizations are visible to everyone with either permission.

---

## Leads

A lead is an inquiry you have not qualified yet. Open **CRM > Leads**. The page has two layouts, **Kanban** and
**List** (switch with the buttons at the top right). The cards above show **Total leads**, **Pipeline** (the
estimated value of open leads), **Promoted** and **Conversion** (promoted leads out of promoted plus
disqualified).

### Lead statuses

| Status | Meaning |
|---|---|
| New | Just arrived. |
| Triaging | Someone is looking at it. |
| Qualifying | You are checking budget, need and timing. |
| Nurture | Not ready now; keep in touch. |
| Disqualified | Not a fit. A reason is required. |
| Promoted | Turned into a deal. Set only by **Promote**. |

A lead can only move between statuses that make sense. For example, a disqualified lead can go back to **New** or
**Nurture**, but not straight to **Qualifying**. If you try a move that is not allowed, the page tells you.

### Add a lead

1. Click **Add lead**.
2. Enter a **Lead title** (for example "Office chairs for new branch, Hoshino Design Office").
3. Pick the **Stage** and the **Source** and, optionally, a **Segment**, an **Estimated value** and its
   **Currency**, an organization and a contact.
4. Click **Create lead**.

Leave the currency empty if you want quotations to use the currency of the customer's country.

### Work a lead

- **Move a lead**: drag its card to another column on the board, or use the card's **Move to** menu. Moving a
  lead to **Disqualified** opens a dialog where you pick the reason. The reasons come from Settings.
- **Edit**: in the list, expand a row and click **Edit**. A panel opens with the lead's details. Change
  **Title**, **Stage**, **Priority**, **Source**, **Segment**, **Est. value**, **Currency** or
  **Notes / description** and click **Save lead**.
- **Add a note**: in the same panel, type in **Add note** and click **Add note**. Notes appear in the
  **Activity log**.
- **Open the conversation**: if the lead's contact has a phone or email, **Open WhatsApp** and **Open email**
  jump to the [CRM inboxes](CRM-Inboxes) with that contact's thread.
- **Score with AI**: click **Score with AI** (or **Re-score**). The AI reads the lead's details and its WhatsApp
  and email history and returns a temperature (**Hot**, **Warm** or **Cold**), a score, the reasons and a
  suggested next step. The score shows on the card and in the **AI score** column.

The list's saved view tabs are **All open**, **Mine**, **Hot**, **No next action**, **Closed** and **All leads**.
A lead with nothing planned shows **No next action** in orange.

### Promote a lead to a deal

Click **Promote** on a lead (on its card, its row or its page). Granary creates a deal at the **Qualifying**
stage with the lead's contact, organization, value, currency, products and source, links the two, and opens the
new deal. The lead's status becomes **Promoted**, and its **Promote** button becomes **Open deal**. Promoting the
same lead twice opens the same deal. Promoting needs `crm:create_deal`.

---

## Deals and the pipeline

Open **CRM > Deals**. The board shows one column per stage; the list view has the saved view tabs **All open**,
**Mine**, **No next action**, **Won**, **Lost** and **All deals**. The cards above show the **Pipeline** value,
**Won** value and **Win rate**.

![Deals pipeline](images/crm-deals-pipeline.png)

### Deal stages

| Stage | Group |
|---|---|
| New, Qualifying, Discovery, Assessment, Proposal, Contract | Open, on the main ladder |
| Won, Handover, Onboarding | Won |
| Nurture | Open, on the side |
| Lost, Disqualified | Closed. One or more reasons are required. |

When a deal enters a stage, its probability is set to that stage's default from Settings. Closing a deal stamps
the close date; reopening it clears the close date and the reasons. A lost or disqualified deal can create a
re-contact reminder, depending on how many months the chosen reason says to wait.

### Create a deal

1. Click **New deal**.
2. Fill in **Title**, **Stage**, **Value**, **Currency** and **Close date**. Leave **Probability** empty to use
   the stage's default.
3. Optionally pick **Product lines**, a **Segment** (leave it empty to follow the lead and the accepted
   quotation) and an **Owner**.
4. Save.

### Move a deal

Drag the card to another column, or use the card's **Move to** menu. Moving a deal to **Lost** or
**Disqualified** asks "Why was this deal lost?" (or disqualified). Pick at least one reason.

### The deal page

Click **Open deal**. The page shows the stage and how long the deal has been in it, **Value**, **Probability**,
**Expected close**, the segment, the organization and the contact, plus the attributes, AI brief and timeline.
If your company has more than one CRM inbox, **Email from** picks the inbox this deal's email is sent from.

The **Quotations** card lists the deal's quotations. Click **New quotation** to start one for this deal, or
**Solution proposal** to start a proposal (see [Sales](Sales)). In the list, **Files** opens the deal's
attachments.

Changing a deal's owner needs `crm:assign_leads`. A user without `crm:view_all` cannot open a deal assigned to
someone else.

---

## Activities

Open **CRM > Activities** to see calls, emails, meetings, tasks, notes and demos across the CRM. The saved views
are **All**, **Today**, **Overdue**, **Open** and **Done**.

To log one:

1. Click **Log activity**.
2. Pick the **Type** (Call, Email, Meeting, Task, Note or Demo).
3. Choose the **Organization** (required) and, optionally, the **Contact**.
4. Enter a **Subject**, the **Scheduled** time, the **Duration** in minutes and **Notes**.
5. Save.

To delete an activity, expand its row and click **Delete**. Deleted activities are not lost: the Owner and admins
can put them back from **Settings > Sales & CRM > Deleted activities**. Logging, editing and deleting activities
needs `crm:log_activity`.

A note longer than the **Longest note** setting is refused, never cut.

---

## Custom attributes

Attributes are extra fields on organizations, contacts, leads and deals. Some are plain fields you fill in; others
are **AI attributes** that the AI fills when you ask.

### Define an attribute

1. Open **Settings > Sales & CRM > CRM attributes**.
2. Pick the **Record type** and click **New attribute**.
3. Choose what it **Applies to**, give it a **Label** (for example "Account tier") and check the **Key**. The key
   is permanent: it is where the value is stored.
4. Pick a **Field type**: text, long text, number, currency, date, select, multi select, checkbox, URL, email,
   phone, rating, user or record reference. For select fields, enter the **Choices**, one per line.
5. To let AI fill it, tick **Fill this field with AI** and pick an **AI kind**:
   - **Summary**: free text written from the record and its history. Give an **Instruction**, for example
     "Summarize the last 90 days of this relationship in two sentences."
   - **Classify**: picks one value against a **Rubric** you write.
6. Click **Create attribute**.

Defining attributes needs `crm:manage_attributes`.

### Use attributes

On a record page, the **Attributes** card lists them. Click a value to edit it. An AI attribute shows a
**Recompute with AI** button and marks its value **Filled by AI**, with the reasoning and sources.

To fill an AI attribute on many records at once, tick the rows in a list (leads, deals, organizations or
contacts), click **Fill with AI** in the bar that appears and pick the attribute. The run happens in the
background; you get a notification when it is done. A selection that is too large is refused.

---

## AI brief

Every record page has an **AI brief** card. Click **Generate** (later **Refresh**) to get a summary of where the
relationship stands, a **Next best action**, **Signals** and **Risks**, based on the record and its full
interaction history. It never runs on its own, because each brief costs an AI call.

---

## AI enrichment and web research

These features look up public facts about a company or person. They need web research to be set up in
**Settings > Nary & AI > Web research**.

- **Quick enrich** (organization page): one web search plus the company's homepage. It fills fields that are still
  empty, in a few seconds. Facts below the confidence threshold are listed as "Low confidence, not written" so you
  can decide yourself.
- **Deep research** (organization page, or the row button on the list): a longer run that finds facts, people and
  news from the web and the company's site. It runs in the background and the results appear in the
  **Web research** panel on the page. People found there can be added as contacts (**Add as contact**) or
  dismissed (**Not relevant**).
- **Research this contact** (contact page or row button): looks for the person's role and LinkedIn profile from
  public profiles.
- **Fill with AI** on a list selection of organizations offers **Deep research** and **Enrich from the web** for
  many records at once.

Granary can also enrich new companies automatically when they are created; see the settings below. Starting
research needs `crm:create_contact` or `crm:edit_contact`.

---

## Follow-up reminders

A follow-up is a reminder to get back to a customer. You set one yourself, or Granary raises one when a
conversation goes quiet.

### Set a reminder

On a lead, a deal, a quotation, or a WhatsApp or email conversation, click **Remind me** (the bell button).

1. Check **What for**.
2. Pick a preset under **When**, or set **Remind me at**.
3. Add a **Note (optional)**, for example "ask about the revised budget".
4. Click **Set reminder**.

Only you see your reminder. It arrives in the bell and on WhatsApp; if it is still open the next day, by email
too. If a reminder already exists for the record, the dialog lists it under **Already set**, with **Done**.

### Automatic reminders

When the rules are on, Granary raises a reminder when:

- the customer has not replied for a number of days after your last WhatsApp or email,
- an open lead has had no activity for a number of days,
- a deal has been stuck in one stage for too long,
- a sent quotation has had no answer,
- a lost or disqualified customer is due to be re-contacted,
- a planned next action falls due.

### The Follow-ups page

Open **CRM > Follow-ups**. The tabs are **Overdue**, **Today**, **Upcoming** and **Done**. Each row shows **Why**,
**Due**, **Note** and the owner. On a row you can:

- **+1 day** or **+3 business days** to snooze it,
- **Done** to close it,
- drop it (the X button),
- **Open record** to go to the lead, deal, quotation or conversation.

With `crm:view_all` you can tick **Whole team** to see everyone's reminders. The Owner also gets a weekly digest
email with everything overdue across the team.

![Follow-ups list](images/crm-followups.png)

---

## Duplicates and merge

Open **CRM > Duplicates**. Click **Scan for duplicates** to look for organizations or contacts that seem to be the
same (pick the tab **Organizations** or **Contacts**). Each suggestion shows a match percentage and the reasons.

To merge a pair:

1. On the record you want to keep, click **Keep this one**.
2. Check the preview: **What moves** (contacts, deals, activities and so on) and **Resulting fields** (fields
   marked "filled from the archived record" come from the other record).
3. Click **Merge permanently**. This cannot be undone.

If the two are different, click **Not a duplicate** and the pair is not suggested again. Nothing is merged until
you confirm it. Merging needs `crm:edit_contact` or `crm:delete_contact`; you must also be allowed to see both records. A record that was
already merged or deleted cannot be merged again.

![Merge preview](images/crm-duplicates-merge.png)

---

## Filtered search

When you ask [Nary](Nary-AI-Assistant) a question about your CRM ("open deals in Proposal over JPY 1,000,000"),
its answer links to a **CRM list** page with the records behind the answer. The filters are shown as chips at the
top. Remove a filter to widen the list, or click **Ask the assistant** to refine the question.

---

## Settings that affect this module

Under **Settings > Sales & CRM**:

- **Funnel** section:
  - **Probability per deal stage** (`crm.funnel.stage_probabilities`): set when a deal enters the stage.
  - **Loss reasons** (`crm.funnel.loss_reasons`) and **Disqualification reasons**
    (`crm.funnel.disqualify_reasons`): the reasons offered, with the months until re-contact.
  - **Lead sources** (`crm.funnel.lead_sources`) and **Lead priorities** (`crm.funnel.lead_priorities`): keys
    already used on leads must stay.
  - **Segments** (`crm.segments`): the segments a lead, deal or catalog item can belong to. Empty means segment
    is free text.
  - **Activity types** (`crm.activity_types`) and **Longest note** (`crm.activity.max_chars`).
  - **Owner of unassigned leads** (`crm.funnel.default_lead_owner_email`): who gets leads that arrive with nobody
    behind them, for example from a web form.
  - **Duplicate deal window (days)** (`crm.deal_dup.window_days`) and **Ask again about a duplicate after (days)**
    (`crm.dup.repropose_days`).
- **CRM attributes**: the custom and AI fields described above.
- **Deleted activities**: restore an activity someone deleted.
- **Follow-up reminders**: the rules (each can be switched off), **Default follow-up delay**, **No reply after**,
  **Lead idle after**, **Deal stuck after**, **Quotation unanswered after**, **Due hour**, **Email if still open
  after**, **Follow-ups per WhatsApp**, the delivery window (**Reminders delivered from / until**, company time
  zone) and the **Weekly digest to the Owner** (weekday and time). Keys start with `crm.followup.`.

Under **Settings > Nary & AI**:

- **AI features**: the master switch and **CRM lead scoring**.
- **Web research**: where research and enrichment get their facts.
- **Company enrichment**: **Research new companies** (`crm.enrich.on_create`), **Fields enrichment may fill**
  (`crm.enrich.fields`), **Minimum confidence** (`crm.enrich.min_confidence`), **Re-research after**
  (`crm.enrich.refresh_days`) and **Daily limit** (`crm.enrich.daily_limit`).
- **Lead-scoring rubric**: how the AI scores leads hot, warm or cold.

---

## Permissions

| Permission | Allows |
|---|---|
| `crm:view_own` | See your own contacts, leads and deals, and all organizations |
| `crm:view_all` | See everyone's contacts, leads and deals; see the whole team's follow-ups |
| `crm:create_contact` | Create contacts and organizations; start enrichment and research |
| `crm:edit_contact` | Edit contacts and organizations; merge or dismiss duplicates |
| `crm:delete_contact` | Delete contacts and organizations; merge duplicates |
| `crm:create_lead` | Create leads |
| `crm:edit_lead` | Edit leads (and deals) |
| `crm:create_deal` | Create deals and promote leads |
| `crm:edit_deal` | Edit deals and move them between stages |
| `crm:delete_deal` | Delete deals |
| `crm:assign_leads` | Change the owner of a lead or deal |
| `crm:log_activity` | Log, edit and delete activities |
| `crm:manage_attributes` | Define custom and AI attributes |

The Owner (superadmin) passes every check.

---

## Related pages

- [CRM Inboxes](CRM-Inboxes): the shared email inboxes and WhatsApp
- [Sales](Sales): quotations, proposals and contracts for your deals
- [Approvals and Signing](Approvals-and-Signing)
- [Marketing](Marketing)
- [Partners](Partners): deal registrations from partners
- [Nary AI Assistant](Nary-AI-Assistant)
- [Configuration](Configuration)

<!-- nav:bottom -->
---

← Previous: [MCP for AI Clients](MCP-for-AI-Clients) | Next: [CRM Inboxes](CRM-Inboxes) →
<!-- /nav:bottom -->
