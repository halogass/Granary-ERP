# Partners

<!-- nav:top -->
[Home](Home) › Using Granary › Sell › **Partners**

**On this page:** [AI in this module](#ai-in-this-module) · [Partner accounts](#partner-accounts) · [Deal registrations](#deal-registrations) · [Partner leads](#partner-leads) · [Commissions](#commissions) · [Marketing development funds (MDF)](#marketing-development-funds-mdf) · [Resources](#resources) · [Training and certification](#training-and-certification) · [The partner portal](#the-partner-portal) · [Settings that affect this module](#settings-that-affect-this-module) · [Permissions](#permissions)
<!-- /nav:top -->

The Partners module runs your channel program: the resellers, distributors and referral partners who sell for
you. Your partner team uses it to manage partner accounts, review the deals partners register, hand warm leads to
partners, record commissions, review marketing development fund (MDF) requests and claims, and publish sales
resources and training. Partners use their own **partner portal** to register deals, follow their leads, request
MDF, read resources and documents, take training and see their commissions.

![Partner accounts list](images/partners-accounts-list.png)

---

## AI in this module

When Nary runs on its own, it helps keep partner work fair. See [AI Across Granary](AI-Across-Granary).

- **Lead routing**: a new lead in a partner's territory (country and vertical) or matching a rule in
  `nary.partners.routing` gets a card proposing that partner.
- **Protection warnings**: you are warned before a deal registration's protection ends (`nary.partners.warn_days`).
- **Conflicts**: for a conflicting registration, Nary gathers the evidence, ERP timestamps first.
- **Partner documents**: Nary offers partner documents only to the contacts of a partner account.

---

## Partner accounts

Open **Partners > Partner Accounts**. The list shows each partner's type, tier, contact, deal value, portal access
and status. Use the saved view tabs (**All**, **Active**, **Gold**, **Platinum**) and **Search partners** to narrow
the list.

### Add a partner

1. Click **Add partner**.
2. Fill in **Company name**, **Partner type** (reseller, system integrator, distributor, referral, technology,
   training or OEM), **Tier**, **Contact name**, **Contact email** and **Country**.
3. Click **Add partner**. The account is created as active.

Tiers rank from lowest to highest: authorized (shown as **Registered**), **Silver**, **Gold**, **Platinum**. The
tier decides which resources and training modules the partner sees (see below).

### Give a partner portal access

1. In the **Portal** column, click **Invite to portal**.
2. Granary creates a portal sign-in for the partner's contact email and emails a temporary password through the
   system SMTP. The partner must change the password at first sign-in.

The column then shows **Invite sent**. If the partner has no contact email, it shows **No email on file**. If the
partner already has an active portal user, it shows **Has access** and no second user is created. Inviting a
partner whose access was revoked reactivates the old user with a fresh temporary password.

### Delete a partner

Click the bin icon on the row, then **Yes** to confirm. Deleting a partner also deactivates its portal users.

### Partner applications from the public sign-up page

Companies can apply on the public page **/partner/register** ("Apply to the partner program"). Applications land in
**Portal users** (open it with Ctrl K, page `/admin/portal-users`) under **Pending registrations**. A system admin
reviews each one and clicks **Approve & send credentials** or **Reject** (a rejection reason is required).
Approving creates the partner account (tier authorized, status pending agreement) and its portal user. Only the
superadmin and users with `system:admin` may approve or reject.

---

## Deal registrations

A deal registration is a partner's claim on an opportunity. While it is protected, the deal counts as the
partner's. Every registration gets **90 days** of protection from the day it is submitted.

Open **Partners > Deal Registrations**. The stats show **Total**, **Pending**, **Approved** and the **Pipeline**
(approved and extended registrations, grouped by currency and never added across currencies). Saved views:
**All**, **Pending review**, **Approved**, **Won**.

![Deal registrations list](images/partners-deal-registrations.png)

### Register a deal for a partner

1. Click **Register deal**.
2. Pick the **Partner**, enter the **Opportunity name**, **End customer**, **Est. value**, **Currency** and
   **Expected close**.
3. Click **Submit**. The registration starts as **Pending**.

Partners can also register deals themselves from the portal (see below).

### Review a registration

1. On a pending row, click **Review**.
2. Choose the **Decision**: **Approve** or **Reject**.
3. When you approve, pick **Our deal**: an existing CRM deal to link, or leave it on **Create a new deal from this
   registration**. A new deal is created at stage "new", owned by you, with a note naming the registration.
4. Add **Review notes** if you like. On a rejection the notes are saved as the rejection reason, which the partner
   sees.
5. Click **Submit**.

Rules the server enforces:

- Only a **pending** registration can be reviewed.
- A CRM deal can belong to only one partner registration. Linking a deal that is already linked to another
  registration is refused.
- Only pending registrations can be deleted.

### Close a registration as won or lost

On an **Approved** or **Extended** row, click **Won** or **Lost** and confirm. A win adds the deal value to the
partner's revenue (this year and cumulative), converted into the partner's commitment currency at the exchange
rate of the closing date. If that rate is missing, the close is refused (see [Finance](Finance)). A registration
with no value cannot be closed as won. Registrations in any other status cannot be closed.

![Review and approve a deal registration](images/partners-deal-review.gif)

---

## Partner leads

Use partner leads to hand a warm lead to a partner to follow up. Open the page with Ctrl K, **Partner leads**
(`/partner-mgmt/leads`). Saved views: **All**, **New**, **Contacted**, **Qualified**, **Converted**.

### Assign a lead to a partner

1. Click **Assign lead**.
2. Pick **Assign to partner** (active partners only) and fill in **Customer name**, **Company**, **Email**,
   **Country**, **Product interest** (comma-separated), **Lead expiry date** and **Notes for partner**.
3. Click **Assign lead**. The lead starts as **New**.

Change a lead's status with the status menu on its row: **New**, **Contacted**, **Qualified**, **Converted** or
**Dead**. The partner sees the lead, its expiry date and your notes in the portal.

Nary can also propose routing a new CRM lead to a partner whose territory (country and vertical) or routing rule
matches it. See [Nary AI Assistant](Nary-AI-Assistant).

---

## Commissions

Open **Partners > Commissions**. The page lists partner commissions with partner, rate, created date, paid date,
amount and status. The stats show **Total**, **Pending**, **Paid out** (grouped by currency) and the number of
**Partners**. Saved views: **All**, **Pending**, **Approved**, **Paid**.

A commission's amount is its base amount times its rate, rounded to the currency's decimals. When a commission is
tied to a deal registration, the base defaults to the registration's value and currency.

A commission moves through these statuses:

| From | To | Who |
|---|---|---|
| Pending | Approved or Cancelled | `partner_mgmt:approve_commission` |
| Approved | Paid or Cancelled | Paid needs `partner_mgmt:pay_commission`; Cancelled needs `partner_mgmt:approve_commission` |

Paid and cancelled are final. The commissions page is read-only: commissions are created and moved through these
statuses through the API (`/api/v1/partner/commissions`), for example by an integration or an MCP client.

---

## Marketing development funds (MDF)

Partners request MDF for a marketing activity, then claim the money they spent against the approved budget. Open
the review page with Ctrl K, **Partner MDF** (`/partner-mgmt/mdf`). It has two tabs, **MDF requests** and
**Claims**, with stats for **Pending review**, **Total approved** and **Pending claims**.

### Review an MDF request

1. On the **MDF requests** tab, click **Review** on a submitted request.
2. Click **Approve** or **Reject**.
3. For an approval, enter the **Approved amount**. For a rejection, enter the **Rejection reason** (required).
4. Click **Confirm approval** or **Confirm rejection**.

Rules: an approved amount must be above zero, cannot exceed the requested amount, and cannot be lower than what
the partner has already claimed.

### Review a claim

1. On the **Claims** tab, click **Review** on a claim. Open **View proof document** to check the evidence.
2. Click **Approve**, **Reject** or **Mark paid**. For **Mark paid**, you can add a **Payment reference**.
3. Confirm.

A submitted claim can be approved or rejected; an approved claim can be marked paid. Partners cannot claim more,
in total, than the approved amount of the request (rejected claims do not count).

![MDF review](images/partners-mdf-review.png)

---

## Resources

Resources are sales kits, datasheets, presentations and other files you share with partners. Open
**Partners > Resources**.

1. Click **Add resource**.
2. Fill in **Title**, **Description**, **Type** (for example Datasheet, Presentation, Case Study, Price List,
   Training, Branding, API Doc), **File URL**, **Version** and **Min tier**.
3. Click **Add resource**.

Use **Deactivate** and **Activate** to hide or show a resource without deleting it. A partner sees an active
resource only when the resource's **Min tier** is at or below the partner's tier and the resource is open to every
partner type or to the partner's own type.

Company documents (brochures, datasheets) with the partner audience come from the document library and appear on
the portal's **Documents** page; each download is stamped with the partner's company. See
[Knowledge and Documents](Knowledge-and-Documents).

---

## Training and certification

Open the training admin page with Ctrl K, **Partner training** (`/partner-mgmt/training`).

### Create a training module

1. Click **New module**.
2. Under **Module info**, enter **Title**, **Description**, **Min tier** and **Pass score (%)** (70 by default).
   Tick **Publish now** to make it visible at once.
3. Click **Add chapter** for each chapter: a title, the content (Markdown) and an optional video URL.
4. In a chapter, click **Add quiz question**, type the question and four options, and select the correct one
   ("The selected option is the correct answer.").
5. Click **Create module**.

Use **Publish** and **Unpublish** on the list to control visibility. Partners see only published modules at or
below their tier.

### How partners take a module

The partner reads each chapter (**Mark read** or **Mark complete**), then takes the **Final quiz**. A score at or
above the pass score passes the module and issues a certification. Before a pass, the partner sees only "Not quite
there yet" and the number of attempts left, never the score, so answers cannot be found by trial and error.

Each partner user gets a limited number of quiz attempts per module (setting **Training quiz attempts**). When they
are used up, the quiz is refused with "Contact your partner manager to try again."

---

## The partner portal

Partners sign in at **/partner/login**. The portal menu has **Dashboard**, **Deals**, **Leads**, **MDF**,
**Commissions**, **Resources**, **Documents** and **Training**. A portal user sees only their own company's data.

![Partner portal dashboard](images/partners-portal-dashboard.png)

- **Dashboard**: the deal pipeline, **Commission this year**, **New leads**, **MDF approved** and modules
  certified, with shortcuts to **Register deal**, **View leads** and **Request MDF**.
- **Deals**: click **Register deal**, fill in **Opportunity name**, **End customer name**, **Estimated value**,
  **Currency** and **Expected close date**, then **Submit registration**. Rejected registrations show **Notes from
  the partnership team**.
- **Leads**: the leads assigned to the partner, with **Notes from us** and the expiry date. **Register as deal**
  opens the deal registration page.
- **MDF**: click **New request** and fill in **Activity title**, **Activity type**, **Description**, **Requested
  amount**, **Currency**, **Planned date**, **Location** and **Expected outcome**. Once a request is approved, the
  partner submits claims with a **Claim amount**, a description and an optional proof file (**Upload proof**:
  invoice, receipt or campaign report).
- **Commissions**: the partner's commissions with type, rate, amount and status.
- **Resources** and **Documents**: the files the partner may access.
- **Training**: modules with status (**Not started**, **In progress**, **Passed**, **Failed**) and certifications.

![Partner portal deal registration](images/partners-portal-register-deal.png)

---

## Settings that affect this module

Under **Settings > Sales & CRM > Partners** (or search for the label):

- **Partner routing rules** (`nary.partners.routing`): rules that propose a new lead to a partner by region and
  industry.
- **Warn before protection ends** (`nary.partners.warn_days`, default 14): days before a deal registration's
  protection ends that Nary warns you.
- **Training quiz attempts** (`partner.training.max_attempts`, default 5): quiz submissions a partner user gets per
  training module.

Closing a won registration in another currency also needs an exchange rate for the closing date (see
[Finance](Finance)).

---

## Permissions

| Permission | Allows |
|---|---|
| `partner_mgmt:view` | See partner accounts, deal registrations, commissions and training modules |
| `partner_mgmt:manage` | Add and delete partners, send portal invites, register deals, manage leads, resources, training and MDF review |
| `partner_mgmt:approve_deals` | Review, close and delete deal registrations |
| `partner_mgmt:manage_commissions` | Create commissions |
| `partner_mgmt:approve_commission` | Approve or cancel commissions |
| `partner_mgmt:pay_commission` | Mark commissions paid |
| `system:admin` | Approve or reject partner applications in Portal users |

The superadmin passes every check. Portal routes accept only partner portal users and only show their own
partner's records.

---

## Related pages

- [CRM](CRM): the deals that approved registrations link to
- [Sales](Sales)
- [Knowledge and Documents](Knowledge-and-Documents): documents shared with partners
- [Finance](Finance): exchange rates
- [Nary AI Assistant](Nary-AI-Assistant): partner routing and protection warnings

<!-- nav:bottom -->
---

← Previous: [Marketing](Marketing) | Next: [Approvals and Signing](Approvals-and-Signing) →
<!-- /nav:bottom -->
