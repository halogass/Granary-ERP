# Marketing

<!-- nav:top -->
[Home](Home) › Using Granary › Sell › **Marketing**

**On this page:** [AI in this module](#ai-in-this-module) · [Campaigns](#campaigns) · [Content calendar](#content-calendar) · [Email marketing](#email-marketing) · [Settings that affect this module](#settings-that-affect-this-module) · [Permissions](#permissions)
<!-- /nav:top -->

The Marketing module helps your marketing team plan and track campaigns, schedule content across channels, and
send email campaigns to mailing lists. Campaigns record budgets and the leads they bring in. The content calendar
plans posts, articles and videos, and sends external content through approval before it is published. Email
marketing sends paced, personalized emails through a CRM inbox, with the sender's signature and a one-click
unsubscribe in every email.

![Email campaign editor](images/marketing-email-editor.png)

---

## AI in this module

Nary has no marketing tools yet: it can't read or change campaigns, the content calendar or email campaigns. In the chat it can explain how this module works, from the ERP guide built into Granary. See [AI Across Granary](AI-Across-Granary) for what the AI does in the other modules.

---

## Campaigns

Open **Marketing > Campaigns** to see your campaigns across all channels. The saved views are **All**,
**Planned**, **Active**, **Paused** and **Completed**. The cards show **Total**, **Active**, **Total budget** and
**Leads generated**.

### Create a campaign

1. Click **New campaign**.
2. Enter the **Campaign name** (for example "Spring office refresh, Tokyo").
3. Pick the **Objective** (lead generation, brand awareness, product launch, nurturing or retention) and the
   **Channel** (email, social media, WhatsApp, ads, events, referral, SEO or other).
4. Optionally fill in the **Target audience**, **Budget** and **Currency**, **Start date**, **End date** and
   **Description**.
5. Click **Create campaign**.

### Run a campaign

On a campaign row:

- **Activate** starts a planned (or paused) campaign.
- **Pause** pauses an active one.
- **Complete** ends an active one.
- Delete (the bin button) removes a campaign that is not active.

---

## Content calendar

Open **Marketing > Content Calendar** to plan content. Switch between **List** and **Week** (use **Previous
week** and **Next week** to move). Filter by status with the saved views (**All**, **Planned**, **In progress**,
**In review**, **Approved**, **Scheduled**, **Published** and others) and by platform with the chips (LinkedIn,
Instagram, X/Twitter, Facebook, Website, Email, YouTube). The cards show **Total**, **In pipeline**, **Scheduled**
and **Published**.

### Add a content item

1. Click **New content**.
2. Enter the **Title** (for example "Five ways to shorten your sales cycle").
3. Pick the **Type** (blog post, social post, email, video, infographic, press release, whitepaper or other) and
   the **Platform**.
4. Set the **Planned date** and write the **Brief / description** (key messages, audience, call to action).
5. Save.

### Move content forward

Each row has a button for its next status: **Planned**, then **In progress**, then **In review**, and so on up to
**Published**.

External content needs approval before it goes out. Content cannot be marked **Approved** by hand: moving an item
to **Scheduled** or **Published** submits it to the **External content approval** policy. By default, technical
content (blog posts, case studies, whitepapers, datasheets) is reviewed by a technical lead, customer references by
a project manager, and then the Owner approves. Follow the request in [Approvals](Approvals-and-Signing).

---

## Email marketing

Open **Marketing > Email Marketing**. The overview shows **Total subscribers**, **Emails sent** and **Avg open
rate**, with your mailing lists and recent campaigns.

Email marketing is limited to the marketing team and the Owner: users with the **Email marketing** permission
(`marketing:email`), the Team Lead of the Marketing department, and the Owner. Others see "Email marketing is
restricted".

Campaign emails are sent through a [CRM inbox](CRM-Inboxes), so set one up first.

### Mailing lists

1. Click **Mailing lists**, then **New list**. Enter the **List name** (for example "Newsletter subscribers") and a
   **Description**, then **Create list**.
2. Open the list and click **Add contacts**.
   - **Manual entry**: **Email**, **Company**, **First name**, **Last name**, then **Add contact**.
   - **Import CSV**: paste one contact per line as `email, first_name, last_name, company`.

Each contact shows as **Active** or **Unsubscribed**. Adding an address that has unsubscribed from your marketing
email does not subscribe it again.

### Write a campaign

1. Click **New campaign**.
2. On the **Settings** tab, fill in **Campaign details**: the **Campaign name**, the **Email subject**, the **From
   name** and **From email**. Use **Personalize** to insert a recipient field into the subject: **First name**
   (`{{first_name}}`), **Full name**, **Company (recipient's)** or **Email address**.
3. Under **Sender and signature**, pick the **Sender** and keep **Close the email with the sender's signature**
   ticked to end the email with that person's email signature.
4. Under **Target lists**, tick one or more lists.
5. On the **Content** tab, build the email from blocks: **Header Image**, **Heading**, **Paragraph**, **CTA
   Button** (label, color and URL), **Image**, **Divider**, **Spacer** and **Code**. Personalization fields work in
   the text too.
6. Click **Save draft** to keep it.

### Preview

Click **Preview** to see the email exactly as it will be sent, footer included. Switch between **Desktop width**
and **Phone width**. Under **Preview as**, choose the first subscriber of the selected lists or a sample recipient,
to check the personalization.

### Send or schedule

- **Send now** queues the email for every subscribed contact on the target lists.
- **Schedule**, then pick the **Schedule send time** and click **Schedule campaign** (or **Schedule** on a saved
  draft).

A campaign needs at least one list and one content block. It can only be sent from **Draft**; after sending, its
content is read-only. Its status moves through **Scheduled** or **Sending** to **Sent** (or **Failed**).

Granary does not send everything at once. It sends one email at a time with a random pause between sends and stops
for the day at a daily cap (company time zone), then continues the next day. This protects your mail server's
reputation. See the settings below.

### The footer and unsubscribe

Every campaign email ends with:

1. the sender's email signature (or the company line when there is none, or when you untick the signature), and
2. an unsubscribe notice with a link. This part is never optional.

Emails also carry a one-click unsubscribe header, so mail apps can show their own **Unsubscribe** button.

When a recipient unsubscribes, the public page `/unsubscribe` confirms "You have been unsubscribed": the address
will no longer receive any marketing email from your company. The opt-out is global: it applies to every list and
every future campaign, and every send checks it. On the same page the recipient can click **Re-subscribe** if they
changed their mind; that lifts the opt-out and subscribes them to their lists again.

### Campaign report

On the campaign list, click **Report** (or **View report** on the campaign). It shows **Total recipients**,
**Delivered**, **Failed** and **Open rate**, and the list of recipients with their state. Use **Search recipients**
to find one. Opens are counted with a tracking image, so mail apps that block images do not count as opened.

![Campaign report](images/marketing-email-report.png)

---

## Settings that affect this module

Under **Settings > Email & WhatsApp > Email campaigns**:

- **Most emails per day** (`marketing.email_daily_limit`, default 100): counted per day in company time.
- **Shortest gap between sends** (`marketing.email_min_delay_seconds`, default 30) and **Longest gap between
  sends** (`marketing.email_max_delay_seconds`, default 60): the random pause between two emails.
- **Sender lease** (`marketing.queue_lock_ttl_seconds`, default 180): must be longer than the longest gap.

Also under **Settings > Email & WhatsApp**:

- **Email signature**: the company signature layout used for the sender's signature.
- **CRM inboxes**: the mail servers campaigns are sent through.

Under **Settings > Approvals & signing > Approval policies**: the **External content approval** policy for the
content calendar.

Under **Settings > Company & branding > Regional**: **Time zone**, which decides when the daily cap resets.

---

## Permissions

| Permission | Allows |
|---|---|
| `marketing:view` | See campaigns and the content calendar |
| `marketing:manage_campaigns` | Create, change and delete campaigns |
| `marketing:manage_content` | Create, schedule and delete content items |
| `marketing:email` | Use email marketing: mailing lists, email campaigns and sending |

The Marketing department's Team Lead can also use email marketing. The Owner (superadmin) passes every check.
The unsubscribe and re-subscribe links are public and need no sign-in.

---

## Related pages

- [CRM](CRM): the leads campaigns bring in
- [CRM Inboxes](CRM-Inboxes): the inboxes campaigns are sent through
- [Approvals and Signing](Approvals-and-Signing): content approval
- [Configuration](Configuration): mail servers and the email signature

<!-- nav:bottom -->
---

← Previous: [Sales](Sales) | Next: [Partners](Partners) →
<!-- /nav:bottom -->
