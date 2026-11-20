# Collaboration

<!-- nav:top -->
[Home](Home) › Using Granary › Work together › **Collaboration**

**On this page:** [AI in this module](#ai-in-this-module) · [Direct Messages](#direct-messages) · [Email (your personal mailbox)](#email-your-personal-mailbox) · [Video Calls](#video-calls) · [Settings that affect this module](#settings-that-affect-this-module) · [Permissions](#permissions)
<!-- /nav:top -->

Collaboration is where your team talks to each other. It has three parts: **Direct Messages** (one-to-one chats, channels, threads, reactions, mentions, file sharing and signed formal memos), **Email** (your own mailbox, connected over IMAP/SMTP, read and answered inside Granary) and **Video Calls** (self-hosted WebRTC calls with a waiting room for outside guests, screen sharing and recording). Every internal user can use all three. Portal users (partners, vendors) cannot.

All three live under **Messaging** in the sidebar.

![Direct Messages with a channel open](images/collaboration-messaging-channel.png)

---

## AI in this module

- **Your mailbox**: ask Nary "Did I reply to Hoshino last week?" or "What's still unread?". It searches your own
  personal mailbox (and the CRM inbox if you have CRM access). Reading another person's mailbox needs
  `email:read_staff_mailboxes`, and every such read is logged.
- **Nary on WhatsApp**: link your own chat with the company number on **Profile > My WhatsApp chats**. Then write
  to Nary there and it answers like the chat, with your permissions; voice notes are transcribed on your server.
  Cards come back as a numbered list: reply "1" to apply the first, "0" to skip. "Nary new" starts a new
  conversation.

Nary doesn't read channels, direct messages or video calls. See [AI Across Granary](AI-Across-Granary).

---

## Direct Messages

Open **Messaging > Direct Messages**. The left column lists **Channels** and **Direct messages**; the right side shows the open conversation. Messages arrive in real time. A green dot shows who is online, and the header shows who is typing.

### Send a direct message

1. Open **Messaging > Direct Messages**.
2. Type a name in **Search channels and people** and pick the person.
3. Write in the composer and press **Enter** to send. **Shift + Enter** starts a new line.

Opening a conversation marks the messages you received in it as read.

### Create a channel

1. Click **New channel** (the **+** next to **Channels**).
2. Enter a **Channel name** (for example `sales-tokyo`) and, optionally, a **Description**.
3. Under **Visibility**, choose:
   - **Anyone can join** (public): every active employee is added automatically.
   - **Invite only** (private): only members can see it.
   - **Announcement**: everyone joins and nobody can leave. Only a superadmin sees this option.
4. Click **Create channel**. You become the channel's owner.

Public and announcement channels you are not in appear under **Browse** in the sidebar. Click one and click **Join**. You cannot join a private channel yourself: its owner must invite you. The Messaging page has no invite button in this version; an owner can add members through the API (`POST /api/v1/messaging/channels/{id}/invite`).

### Leave or delete a channel

Open the channel and click **More** in the header.

- **Leave channel**: for any channel you are a member of, except announcement channels.
- **Delete channel**: only the person who created the channel or a superadmin. The channel is archived, not erased.

Only channel members can read and post messages in a channel. A private channel is invisible to non-members, including in search.

### Reply in a thread

1. Hover a message and click **Thread**.
2. The thread panel opens on the right. Write in **Reply in thread** and press **Enter**.

A message with replies shows a link such as **3 replies**; click it to open the thread. Threads work in both channels and direct messages.

### React with an emoji

Hover a message and click **Add reaction**, then pick an emoji. Click the same emoji again to remove your reaction. Each reaction shows how many people chose it.

### Mention people and records

- Type **@** and start a name to mention a colleague. The person gets a notification.
- Type **#** to link a record: a project, a deal, an SOP article or a product. Use **Mention entity type** in the dropdown to narrow the list.

### Share files

Click **Attach files** in the composer, or drop files onto it. You can send:

| Type | Limit |
|---|---|
| Images | 5 MB |
| Videos | 100 MB |
| Documents (PDF, Word, Excel, PowerPoint, text, CSV, Markdown, JSON, ZIP) | 25 MB |

Other file types are refused with "Files of type '...' can't be sent here." Files are kept in Granary's private S3 storage and are linked fresh each time someone opens the message.

Click **Insert emoji** in the composer to add an emoji to your text.

### Search a conversation

Click **Search** in the conversation header and type in **Search messages**. Matching messages are listed and highlighted.

### Send a formal memo

A formal memo is a numbered, digitally signed notice that recipients must acknowledge. Use it for instructions you need a record of, for example "Submit Q2 project reports by 30 May 2026".

1. Open the direct message or channel the memo is for.
2. Click **Memo** in the header (**Send formal memo**).
3. In **Compose formal memo**, check **To**, then fill in **Subject**, **Memo type** and **Memo content**:
   - **Directive**: an order or instruction that must be followed.
   - **Information / notice**: for awareness, no action required.
   - **Request**: formally requesting something.
4. Click **Send signed memo**.

![A formal memo card in a channel](images/collaboration-memo-card.png)

The memo shows as a card with its status: **Pending acknowledgement**, **Partially acknowledged**, **Acknowledged** or **Recalled**.

- **Acknowledge**: the recipient of a direct memo, or any channel member other than the sender, clicks it once. The card lists who acknowledged and when.
- **Recall**: only the sender, and only before anyone has acknowledged. Recipients then see it as recalled.
- **PDF**: downloads the signed memo, with the acknowledgements, as a PDF.

You can only send a memo to a channel you belong to.

### Start a call from a conversation

In a direct message, click **Voice call** or **Video call** in the header. Granary creates a call room, notifies the other person and opens the room in a new tab. See [Video Calls](#video-calls) below.

---

## Email (your personal mailbox)

**Messaging > Email** connects your own mailbox (for example `aiko.tanaka@example.com`) to Granary. Each user connects their own accounts; nobody else can see them. This is separate from the shared CRM inboxes described in [CRM](CRM).

![The personal email client with a message open](images/collaboration-email-inbox.png)

### Connect an account

1. Open **Messaging > Email** and click **Add account**.
2. Fill in **Display name** (defaults to the email address), **Email address**, **Username** and **Password or app password**.
3. Fill in **IMAP host**, **IMAP port** with **SSL**, and **SMTP host**, **SMTP port** with **TLS**. Your mail provider publishes these values.
4. Click **Add account**.

The password is stored encrypted and is never shown again. To change account details, click **Edit account**; leave the password blank to keep the current one. **Remove account** deletes the account and the copies of its emails held in Granary (not the emails on your mail server).

### Read and sort email

- Pick a folder: **Inbox**, **Sent**, **Drafts**, **Archive**, **Spam** or **Trash** (only the folders your server has are shown).
- Use **Search email**, **Unread**, **Starred** and **Has attachments** to filter the list.
- Click a message to read it. Opening a message marks it read in Granary and on the mail server.
- **Mark as read**, **Mark as unread** and **Star** also sync back to the server.
- **Load older emails** fetches the next batch of older messages from the server.
- Attachments: click the file name to open it, or **Download** to save it.

Granary checks every connected mailbox every 5 minutes (Inbox, Sent and read status changed elsewhere). Click **Sync inbox** to check now.

### Compose and reply

1. Click **Compose**, or **Reply to ...** under an open message.
2. Choose the account to send from, then fill in **To**, **Subject** and the message.
3. Click **Send**.

Your personal email signature (set on your **My profile** page) is added to the body when the composer opens, so you see what will be sent. A copy goes into the server's Sent folder.

### Act on many emails at once

Tick emails (or **Select all loaded emails**, then the option to select every email in the folder) and choose **Mark as read**, **Mark as unread**, **Star**, **Unstar** or **Delete**. One action can cover at most 2,000 emails; narrow the selection if more match.

**Delete** moves emails to the server's Trash. In **Trash**, the button becomes **Delete forever**, which cannot be undone.

### Link an email to a CRM contact

When a customer writes to your personal address, click **CRM context** on the open message. The panel lets you link the sender to a CRM contact. The link applies to that sender's other emails in your mailboxes too, so the conversation shows up on the contact.

---

## Video Calls

**Messaging > Video Calls** runs calls on your own server. Media goes peer to peer over WebRTC; when a direct connection is not possible (strict firewalls, mobile networks), traffic is relayed through the TURN server (coturn) that ships with Granary.

![A video call with three participants and the waiting room open](images/collaboration-call-in-progress.png)

### Start a call

1. Open **Messaging > Video Calls** and click **Start new call**.
2. On the **Ready to join?** screen, check your camera and microphone.
3. Optional: under **Invite team members**, search and add colleagues (**Pre-invite team members**). They get a notification and are admitted automatically when the call starts.
4. Optional: copy the link under **Share this link with external guests** and send it to people outside the company. **Copy room ID** copies the short room ID for colleagues.
5. Click **Join call**.

A call holds up to 10 participants at a time. A colleague who has the room ID can type it in **Enter room ID** and click **Join**; invited colleagues can also open the call from their notification.

### During the call

- **Mute mic** / **Unmute mic**, **Stop camera** / **Start camera**, **Share screen** / **Stop sharing**.
- **Chat**: text chat with everyone in the call.
- **Invite to this call**: add more colleagues while the call runs.
- **Share link**: copy the guest link again.
- **Remove from call**: the host removes a participant.
- **End call**: the host (the person who started it) or a superadmin ends the call for everyone.

### Admit outside guests

A guest opens the link (`/meet/<room-id>`), enters **Your name** and, optionally, **Email**, and clicks to request to join. The guest sees "Waiting for host" until you decide.

1. When a guest is waiting, open **Waiting room** in the call.
2. Click **Admit** or **Reject** next to the guest.

Admitted guests check their camera on a lobby screen and then join. Only the host can see the waiting room and admit or reject guests. If the guest's email matches a CRM contact, the waiting room shows the contact; if it matches a colleague, they are labelled **Internal user**. The number of guests who can wait at once is a setting (see below). When it is reached, new guests see "The waiting room is full. Try again in a few minutes."

![Guest admission flow](images/collaboration-guest-admit.gif)

### Record a call

1. As the host, click **Start recording**. A **Recording** badge shows in the call.
2. Click **Stop recording**. The recording is uploaded to Granary's S3 storage (shown as **Uploading**).
3. Click **Recording** to download it as a WebM video.

The recording is made in the host's browser: it combines the video tiles on the host's screen and everyone's audio. Only the host can upload a recording. The host, the call's participants and superadmins can download it; the download link is valid for 2 hours.

---

## Settings that affect this module

Settings are under **Settings > General**.

| Setting | Where | What it does |
|---|---|---|
| Email signature (`messaging.signature-template`) | Email & WhatsApp > Email signature | The layout every staff signature uses; each user fills in their own on **My profile** |
| Lock per account folder (`mail.sync_lock_ttl_seconds`) | Email & WhatsApp > Mail sync | How long one mailbox sync holds a folder before another sync may start |
| Guests waiting per call (`calls.guest_wait.max_waiting`) | System & security > Public pages | The most guests waiting to be admitted to one video call |
| Public form submissions per minute (`public.forms.requests_per_minute`) | System & security > Public pages | Rate limit per address, including joining a call's waiting room |

Calls also need server configuration (environment variables, see the deployment docs): `TURN_SECRET`, `TURN_URIS` and `TURN_EXTERNAL_IP` for the TURN server, `SIGNAL_URL` for the public signalling WebSocket and `SIGNAL_TICKET_KEY` to sign call tickets. File uploads and recordings use the S3 storage settings (the names date from the earlier MinIO setup: `MINIO_ENDPOINT`, `MINIO_ACCESS_KEY`, ...).

## Permissions

Direct Messages, Email and Video Calls have no permission codes: every signed-in internal user can use them, and portal users cannot. The rules inside the module are:

- Announcement channels: created only by a superadmin.
- Private channels: readable only by members; only the channel owner can invite.
- Deleting a channel: its creator or a superadmin.
- Email accounts and emails: only the user who connected them.
- Calls: the host or a superadmin ends the call, removes participants, manages the waiting room and uploads recordings. Guest join pages need no sign-in.

## Related pages

- [CRM](CRM) (shared CRM inboxes and contacts)
- [Projects](Projects) (projects you can mention with #)
- [Knowledge and Documents](Knowledge-and-Documents) (SOP articles you can mention with #)
- [HR and Payroll](HR-and-Payroll) (employees and positions shown on memos)

<!-- nav:bottom -->
---

← Previous: [Knowledge and Documents](Knowledge-and-Documents) | Next: [Configuration](Configuration) →
<!-- /nav:bottom -->
