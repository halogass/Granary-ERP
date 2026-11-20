# CRM Inboxes

<!-- nav:top -->
[Home](Home) › Using Granary › Sell › **CRM Inboxes**

**On this page:** [AI in this module](#ai-in-this-module) · [Before you start](#before-you-start) · [The email inbox](#the-email-inbox) · [Scheduled sends](#scheduled-sends) · [The WhatsApp inbox](#the-whatsapp-inbox) · [Set up a CRM inbox](#set-up-a-crm-inbox) · [Settings that affect this module](#settings-that-affect-this-module) · [Permissions](#permissions)
<!-- /nav:top -->

The CRM inboxes are where your team talks to customers: the shared company email inboxes (for example
`hello@` and `sales@`) and a shared WhatsApp inbox. Every conversation is linked to the CRM, so you see the
customer's contact, organization, leads, deals and quotations next to the messages. Sales and support staff use
these inboxes instead of a separate mail client or phone. Replies can be sent now or scheduled for later.

These are company inboxes, shared by everyone with access. Your personal mailbox is a different feature (see
[Collaboration](Collaboration)).

![CRM email inbox](images/crm-inboxes-email.png)

---

## AI in this module

The inboxes are where Nary meets your customers. You can use the AI on a single thread, and the Owner can let
Nary work the inboxes on its own, always with cards for anything that goes to a customer. See
[AI Across Granary](AI-Across-Granary).

- **Draft a reply with AI**: in an email or WhatsApp thread, click the AI button, say what the reply should do (or
  pick a suggestion such as "Propose a meeting") and draft. The draft lands in the reply box; you edit and send it
  yourself. Nothing is sent by the AI.

  ![Drafting an email reply with AI and editing it before sending](images/crm-inboxes-ai-reply.gif)

- **Ask AI about an attachment**: on a PDF or image in an email, click the sparkle button (**Ask AI about this
  document**), then **Analyze**. Ask a question, or leave it empty to get a summary and the key fields.
- **Nary at the front door** (when Nary runs on its own): each new WhatsApp chat or email is sorted (triage),
  matched to a contact, and a new prospect becomes a lead. Web form notifications become leads with every field
  kept. Photos and PDFs are described and voice notes are transcribed on your own server. Chats from numbers the
  CRM doesn't know are held, and nothing reads them until the Owner decides **business** or **personal** on a
  card.
- **Replies to customers**: Nary drafts a reply to each new customer message (an answer first, at most two
  questions, list prices only, meeting times when asked) and puts it in **Nary > Approvals** as a card. With
  shadow mode on (the default) the reply is only a draft that Nary compares with what you actually send. When a
  customer answers, the Owner gets a short reply alert.
- **From the chat**: ask Nary to write to a customer ("Send Ms. Sato the price list on WhatsApp") and it prepares
  the message, with attachments and an optional send time, as a card (needs `nary:approve`). Linking a chat or an
  email thread to a contact, lead or deal is also a card.

---

## Before you start

- **Email**: an administrator adds at least one CRM inbox in **Settings > Email & WhatsApp > CRM inboxes**, with
  its outgoing (SMTP) and incoming (IMAP) servers. See [Set up a CRM inbox](#set-up-a-crm-inbox).
- **WhatsApp**: Granary does **not** ship a WhatsApp bridge. The WhatsApp inbox talks to an HTTP service that you
  (the person installing Granary) provide at `WHATSAPP_SERVICE_URL`, with the token `WHATSAPP_SERVICE_TOKEN`.
  Without one, **CRM > WhatsApp** shows "WhatsApp not connected" and everything else in Granary keeps working.
  See [Configuration](Configuration).

---

## The email inbox

Open **CRM > Email Inbox**. The left pane lists conversations (threads); the right pane shows the open thread.

### Find a conversation

- Switch between **Inbox** (conversations with mail from customers) and **Sent** (conversations you started,
  from Granary or from any other mail client, because the mailbox's Sent folder is synced too).
- If there is more than one inbox, pick one in the inbox selector, or keep **All inboxes**.
- Use the status chips **All**, **Open**, **Active** and **Resolved**, and **Unread** to show only unread
  conversations.
- Type in **Search threads**.
- Click **Sync now** to fetch new mail right away. The list also refreshes every minute.
- At the bottom of a thread, **Load older emails** reaches back into the mailbox for history from before the
  first sync.

Opening a conversation marks it read for everyone, because the inbox is shared. Read status is also synced with
the mail server in both directions. Use **Mark as unread** to put it back.

### Reply, reply all or forward

1. Open the thread. Press **R** or click the folded reply bar at the bottom.
2. Pick **Reply**, **Reply all** or **Forward**.
3. Check the address line. Click it to show **From**, **To**, **Cc**, **Bcc** and **Subject**; they are prefilled
   from the thread, and you can add or remove anyone.
4. Write your message. Click **Attach** to add files, or drag them onto the reply box.
5. Optional: click the AI button (**Draft a reply with AI**). Say what the reply should do, or pick a suggestion
   such as "Propose a meeting", then draft. Leave it empty for a plain suggestion.
6. Click **Send reply** (or **Send forward**), or press **Ctrl+Enter**.

Your email signature is added under your text when you send. You can see it folded under the editor. The reply
box can be resized, popped out for a long message (**Pop out the reply**) and folded with **Esc**; the draft is
kept while it is folded.

Sent mail is also saved in the mail server's Sent folder, so the customer's reply threads onto it.

### Link a thread to the CRM

The **CRM context** panel on the right shows who the sender is in the CRM:

- If the sender is a known contact, you see the contact, organization, leads, deals, quotations and recent
  activity. You can add a note with **Log a note**.
- If not, **Link existing contact** finds a contact, or **Create contact** adds one. For a new contact, Granary
  suggests the organization from the sender's company email domain (not for free mail providers).

The **Link to CRM** button in the thread header links the whole thread to a **Contact**, **Organization**,
**Lead** or **Deal**. Threads that arrived before a contact existed are linked to it automatically later, and
their emails are logged on its history.

### Track status and assign

- Set the thread's status in **Thread status**: **Open**, **In progress** or **Resolved**. A resolved thread shows
  **Reopen**.
- To act on many threads, tick them (or **Select all loaded conversations**). The bar that appears offers
  **Read**, **Unread**, **Status**, **Assign** (type a colleague's name, or **Unassign**) and, for the Owner and
  system admins, **Delete**. The person you assign gets a notification that opens the conversation.

### Delete

Only the Owner and system admins can delete. **Delete conversation** moves every email of the thread to the
mailbox's Trash on the mail server. A single email has its own delete button, which also moves it to the Trash.

---

## Scheduled sends

You can write a message now and have Granary send it later, by email or WhatsApp.

1. Write your reply as usual.
2. Click **Schedule send** (the clock button next to Send).
3. Pick a preset (**Tomorrow 9am**, **This afternoon 2pm**, **Next Monday 9am**) or set a **Custom time**, then
   click **Schedule**.

The scheduled message appears in the thread as pending, with **Edit** and **Cancel**. You can only edit or cancel
a message that is still pending, and only your own. The time must be in the future.

If a send fails, Granary retries it a set number of times, then marks it failed. A send that is stuck for too long
is also marked failed (see the settings below).

Scheduling a WhatsApp message needs `crm:send_whatsapp`, the same as sending it now.

---

## The WhatsApp inbox

Open **CRM > WhatsApp**. You need a connected WhatsApp bridge (see [Before you start](#before-you-start)).

![WhatsApp inbox](images/crm-inboxes-whatsapp.png)

### Connect the company WhatsApp account

Only the Owner and system admins see the QR code.

1. Click **Scan QR to connect** (or **Connect WhatsApp** in the empty pane).
2. On the company phone, open WhatsApp, tap **Menu** or **Settings**, then **Linked Devices**, then
   **Link a Device**.
3. Scan the QR on screen. It refreshes about every 20 seconds.
4. When it says **Connected**, click **Done**.

Other users see "WhatsApp is not connected. Ask an admin to set it up." until it is linked.

### Read and send

- Search with **Search chats**. **Load older chats** widens the list.
- Open a chat. **Load older messages** loads more; when there is nothing more in Granary, **Load older from
  phone** asks the phone for older messages (the phone must be online).
- Type in **Type a message** and click **Send**. Use **Attach file** for a picture or document (with an optional
  **Caption**) and **Emoji** for emoji.
- Use a message's **Reply** button to quote it, or **Forward** to send it to another chat.
- The AI reply suggestion works as in email.
- **Schedule send** works as described above.

Every message you send carries your **WhatsApp signature**, so customers know who is replying. Set it on your
profile page (**My profile > WhatsApp signature**). Staff can shorten the name in it; only the Owner can replace
the whole line with their own text.

Images and files are loaded one by one. If one shows "Image unavailable", click **Retry**; if it says "re-sync this
chat on your phone", the phone no longer has it.

### CRM context, export and delete

- **CRM context** shows the same panel as in email: link the number to a contact, create one, and see the
  contact's leads, deals and quotations.
- **Export conversation** downloads a transcript, as **PDF** (branded, images inline) or **Text** (plain .txt,
  no media). Leave **From** and **To** empty to export everything WhatsApp still has for the chat.
- **Delete chat** (Owner and system admins only) removes the chat from this list only. It stays on the phone and
  comes back if the customer writes again.
- Tick several chats to **Mark read** them together (or **Delete**, for the Owner and system admins).

---

## Set up a CRM inbox

This is for administrators.

1. Open **Settings > Email & WhatsApp > CRM inboxes**.
2. Under **Inboxes**, click **Add inbox**. Enter the **Address** (for example `sales@example.com`), a **Display
   name** (for example "Kumo Trading Sales"), the **Purpose** (General or Sales) and **History to import** (days
   of past mail, read once).
3. Fill in the SMTP and IMAP servers and passwords, and use the **Send test** buttons (they send to the **Test
   recipient** set in the same area).
4. The inbox is switched on once its servers are set and tested. It then imports its recent mail quietly.
5. Click **Make default** on the inbox that should send for deals that have no inbox of their own.

Passwords are stored encrypted in the database. A deal's own inbox is chosen on the deal page (**Email from**,
see [CRM](CRM)).

---

## Settings that affect this module

Under **Settings > Email & WhatsApp**:

- **Email signature**: the company signature layout every staff signature uses (each user fills in their own
  details).
- **Test recipient**: where the **Send test** buttons send to.
- **CRM inboxes**: the inboxes and their servers (`crm_smtp_*`, `crm_imap_*`), the default inbox, and:
  - **Link threads of the last (days)** (`crm.email.relink_days`): how far back threads are linked to a newly
    created contact.
  - **Link threads every (minutes)** (`crm.email.relink_interval_minutes`) and **Full link sweep every (hours)**
    (`crm.email.relink_full_hours`): how often that linking runs.
- **WhatsApp link**:
  - **Reconnect automatically** (`wa_session_auto_reconnect`): when the WhatsApp session drops.
  - **Keep the chat list for** (`whatsapp.conversations_cache_minutes`): when WhatsApp is slow, the inbox shows
    the last list it fetched (0: never).

Under **Settings > Sales & CRM > Scheduled messages**:

- **Attempts before failing** (`crm.scheduled.max_attempts`).
- **Fail a send stuck for** (`crm.scheduled.stuck_after_minutes`).

Under **Settings > Nary & AI > AI features**: **Reply and draft suggestions** switches the AI reply button on or
off.

---

## Permissions

| Permission | Allows |
|---|---|
| `crm:view_all` or `crm:view_own` | Read the CRM email inbox |
| `crm:send_email` | Reply and send from the CRM inboxes, sync, load older mail from the server |
| `crm:view_whatsapp` | Read the WhatsApp inbox |
| `crm:send_whatsapp` | Send and schedule WhatsApp messages |
| `crm:edit_contact` | Link a conversation to a contact |

Deleting conversations and chats, and pairing WhatsApp, are for the Owner (superadmin) and system admins. Portal
users (partners, vendors) never see the WhatsApp inbox.

---

## Related pages

- [CRM](CRM)
- [Sales](Sales): quotations sent by email or WhatsApp
- [Approvals and Signing](Approvals-and-Signing): sending approved documents
- [Marketing](Marketing): email campaigns go out through a CRM inbox
- [Configuration](Configuration): mail servers and the WhatsApp bridge
- [Collaboration](Collaboration): your personal mailbox

<!-- nav:bottom -->
---

← Previous: [CRM](CRM) | Next: [Sales](Sales) →
<!-- /nav:bottom -->
