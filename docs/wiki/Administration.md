# Administration

<!-- nav:top -->
[Home](Home) › Administration › **Administration**

**On this page:** [The Owner](#the-owner) · [Users](#users) · [Roles and permissions](#roles-and-permissions) · [Portal users](#portal-users) · [Two-factor sign-in](#two-factor-sign-in) · [Sessions and sign-in](#sessions-and-sign-in) · [Security features](#security-features) · [Reporting a security problem](#reporting-a-security-problem) · [Settings that affect administration](#settings-that-affect-administration) · [Permissions](#permissions)
<!-- /nav:top -->

This page is for the Owner and system admins. It covers user accounts, roles and permissions, portal users
(vendors and partners), two-factor sign-in, sessions, and the security features you should know about. Settings
in general are on [Configuration](Configuration).

![Settings > Users with the user list](images/administration-users.png)

---

## The Owner

The first account, created from `SUPERADMIN_EMAIL` on the first start, is the **Owner** (the superadmin). The
Owner:

- holds every permission (the `superadmin` role);
- is the default approver in the shipped approval policies (every invoice, every non-catalog quotation, large
  purchase orders and expenses);
- receives Nary's cards, briefs and system alerts (placeholder secrets, failed jobs);
- is the only person who can grant the superadmin role, change the Owner's access or reset the Owner's password;
- can't be deactivated or deleted.

Keep the Owner account for the person who runs the company, and turn on two-factor sign-in for it.

---

## Users

Open **Settings > Users** (or **Settings > Integrations & access > Users & roles > Manage users**). The list shows
each user's portal type and roles, department, linked employee, last sign-in and status. Use **Show deactivated**
to include deactivated accounts.

### Add a user

1. Click **Add user**.
2. Fill in **Full name**, **Email** and **Password** (at least 8 characters).
3. Pick the **Portal type**:
   - **Internal (staff)**: employees who use the ERP;
   - **Partner**: partner contacts, who see only the partner portal;
   - **Vendor**: vendor and contractor contacts, who see only the vendor portal.
4. Optional: **Phone**, **Department**, **Position**, and the **employee record** to link.
5. Tick **System administrator** only for people who should manage all settings and users.
6. Click **Create user**, and give the person their password through a safe channel.

Link every staff user to their employee record. The link connects the sign-in to attendance, leave, payroll and
time logs.

### Edit, reset, deactivate

On a user's row:

- **Edit user**: name, phone, portal type, department, position, **WhatsApp signature name**, **System admin**,
  **Active**.
- **Reset password**: sets a new password. It ends all of that person's sessions.
- **Deactivate**: the person can no longer sign in. Their linked employee record is marked as terminated so HR
  data stays consistent. **Reactivate** brings the account back.
- **Two-factor sign-in**: shows whether the person uses it; an admin can reset it when someone loses their phone.
  The person then sets it up again in their profile.

### Rules the server enforces

- Only a system admin (or someone with `system:manage_users`) can create users.
- A user manager who is not a system admin can't create a system admin.
- Everyone except the Owner can hand out only roles whose permissions they hold themselves. The request is
  refused with a message naming the missing permissions.
- Only the Owner can grant the superadmin role.

---

## Roles and permissions

Access in Granary is a set of **permissions** (for example `crm:view_all`, `sales:create_quotation`,
`finance:post_journal`), grouped into **roles**. A user's effective permissions are the union of their roles.
The menu hides what a person can't use, and the server checks every request again.

The seed creates these roles:

| Role | Intended for |
|---|---|
| `superadmin` | The Owner: every permission |
| `executive` | Senior management; also allowed to edit the product catalog |
| `system_admin` | Settings, users and roles |
| `team_lead` | Team leads across areas |
| `sales_bizdev` | Sales and business development |
| `marketing` | Marketing |
| `engineer` | Engineers |
| `project_manager` | Project managers |
| `finance_admin` | Finance and accounting |
| `hr_admin` | HR |
| `manufacturing_tech` | Production and QC |
| `inventory_controller` | Stock and warehouses |
| `maintenance_facilities_tech` | Maintenance and facilities |
| `helpdesk_support` | Helpdesk |
| `partner_portal`, `vendor_portal` | Portal users (no internal permissions) |

Each user-guide page lists the permissions that gate its module under **Permissions**.

The user list shows each person's roles. Assigning and removing roles, and creating a custom role from a set of
permission codes, are done through the API (`POST /api/v1/users/{id}/roles/{role}`,
`DELETE /api/v1/users/{id}/roles/{role}`, `POST /api/v1/roles`, system admin only); the users page itself does
not have role controls. `GET /api/v1/roles/permissions` lists every permission with its description.

**System admin** is a flag on the user, separate from roles: it gives access to all settings, users and roles.

---

## Portal users

Vendors and partners sign in at the same `/login` page as staff and land in their own portal (`/vendor/dashboard`
or `/partner/dashboard`). They never see the internal ERP.

Open **Portal users** (press **Ctrl K** and type "Portal users", or go to `/admin/portal-users`). The page has
three tabs: **Pending approval**, **Vendor users** and **Partner users**.

- **Approve a registration.** Vendors and partners can register themselves from the public sign-up. Open a
  pending registration with **Review**, then **Approve & send credentials**: Granary creates the vendor or partner
  and its user account and emails the sign-in details. To refuse, enter a **Rejection reason** and click
  **Reject**.
- **Create a portal account yourself.** Use **Create portal account**, choose the **Portal type**, then
  **Create & send credentials**.

Approving and creating portal accounts needs the Owner or the `system:admin` permission. Public sign-ups are
rate-limited per address (`public.forms.requests_per_minute`).

---

## Two-factor sign-in

Two-factor sign-in (TOTP) asks for a 6-digit code from an authenticator app after the password. Any user, portal
users included, can turn it on.

To turn it on:

1. Open **My profile** and find the **Two-factor sign-in** card.
2. Enter **Your password** and click **Turn on**.
3. Scan the QR code with an authenticator app (or type the **Setup key**).
4. Enter the code in **2. Code from the app** and click **Confirm and turn on**.
5. Save the **Recovery codes** that appear (one-time codes for when you lose your phone), then click
   **I saved them**.

With it on, the card shows how many recovery codes are left. **New recovery codes** replaces them; turning it off
needs a current code or a recovery code.

![The Two-factor sign-in card on My profile](images/administration-two-factor.png)

Two-factor sign-in also protects other actions:

- people whose role approves sensitive actions are asked to turn it on, and each such approval asks for a code;
- creating an MCP token or connection that can act asks for the code (see
  [MCP for AI Clients](MCP-for-AI-Clients));
- AI clients at full power need the code to apply customer messages, invoices, meetings, money changes, deletes
  and merges (`mcp.apply.require_totp`).

**Turn it on for the Owner, every system admin and everyone who connects an AI client.**

Settings (**Settings > System & security > Login security**):

| Setting | What it does |
|---|---|
| Two-factor: name in the app (`auth.totp.issuer`) | What authenticator apps show for these accounts |
| Two-factor: time to enter the code (`auth.totp.challenge_seconds`) | How long after the password the code is accepted |
| Two-factor: wrong codes allowed (`auth.totp.max_failed_attempts`) and wrong-code window (`auth.totp.failed_window_seconds`) | Per person; wrong codes at sign-in also count toward the sign-in limit |
| Two-factor: recovery codes (`auth.totp.recovery_codes`) | How many one-time codes are made |

---

## Sessions and sign-in

- A sign-in gives a short-lived access token (60 minutes) and a refresh token (30 days), renewed automatically
  while the browser is open.
- **Signing out** ends that whole sign-in, including access tokens issued from it a few minutes earlier.
- **Changing your password** (My profile > **Change password**) ends every other session at once; the device you
  used stays signed in. It also disconnects all your MCP tokens and AI client connections.
- **An admin resetting a password** ends all of that person's sessions.
- **Failed sign-ins** are limited per address (`auth.login_max_attempts` per `auth.login_window_seconds`, default 5
  per 15 minutes), plus Nginx's own limit on the sign-in path. People behind one office address share it.
- Changing `JWT_SECRET_KEY` or `NEXTAUTH_SECRET` signs everyone out.

---

## Security features

- **Placeholder secrets are refused or reported.** The API and the worker refuse to start while `JWT_SECRET_KEY`,
  `APP_SECRET_KEY` or `NEXTAUTH_SECRET` still holds the `.env.example` placeholder. Other weak secrets, in `.env`
  or in Settings, are logged by name (never by value) and the Owner gets one alert per start.
- **Secrets in Settings are encrypted** (mail passwords, AI and API keys, tokens).
- **Grant only what you hold.** Nobody but the Owner can hand out a role with permissions they don't have.
- **Portal users are kept out of internal modules**, including the role list and MCP tokens.
- **WebSocket tickets.** Live connections (messaging, WhatsApp, calls) never put the access token in the URL,
  where it could end up in logs. The browser first gets a single-use ticket, valid for 30 seconds and bound to
  its session, and opens the WebSocket with that.
- **AI clients get explicit consent.** Connecting an outside AI client at a level that can act needs your password
  and, with two-factor sign-in on, your authenticator code. Such connections end after
  `mcp.pat.max_days_power` days (default 90), however often they refresh. See
  [MCP for AI Clients](MCP-for-AI-Clients).
- **Sidecars and data services are internal.** The database, Redis, the storage and the sidecars are reachable
  only on the internal Docker network; host ports are bound to `127.0.0.1`.
- **The storage is private by default.** Only the public bucket (logos, public images) can be read without
  credentials. Private files reach the browser through short signed links.

---

## Reporting a security problem

Report security problems privately, never in a public issue, pull request or discussion. Open the repository's
**Security** tab and choose **Report a vulnerability**. Include the component and commit, what an attacker can do
and what access they need first, the steps to reproduce, and what you expected. Test only against your own
installation. Security fixes go to `main` only.

---

## Settings that affect administration

| Setting | What it does |
|---|---|
| `auth.login_max_attempts`, `auth.login_window_seconds` | Failed sign-ins per address per window |
| `auth.totp.*` | Two-factor sign-in (see above) |
| `public.forms.requests_per_minute` | Public form submissions per minute from one address (portal sign-ups, support tickets, call waiting rooms) |
| `mcp.pat.max_days_power` | Longest life of an AI client connection that can act |

## Permissions

- `system:manage_users`: create and edit users.
- `system:admin` (or the System administrator flag): settings, roles, portal users, resetting two-factor sign-in.
- `system:manage_roles`, `system:manage_settings`, `system:view_settings`, `system:view_audit_log`.
- `system:superadmin`: the Owner.

## Related pages

- [First Steps](First-Steps)
- [Configuration](Configuration)
- [MCP for AI Clients](MCP-for-AI-Clients)
- [Procurement](Procurement) (vendor portal)
- [Partners](Partners) (partner portal)

<!-- nav:bottom -->
---

← Previous: [Configuration](Configuration) | Next: [Backups and Upgrades](Backups-and-Upgrades) →
<!-- /nav:bottom -->
