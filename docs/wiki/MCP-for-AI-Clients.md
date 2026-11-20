# MCP for AI Clients

<!-- nav:top -->
[Home](Home) › AI in Granary › **MCP for AI Clients**

**On this page:** [Access levels](#access-levels) · [Connect with a browser login (OAuth)](#connect-with-a-browser-login-oauth) · [Connect with a personal access token](#connect-with-a-personal-access-token) · [See and revoke connections](#see-and-revoke-connections) · [How changes are applied](#how-changes-are-applied) · [Tips for AI clients](#tips-for-ai-clients) · [Settings that affect MCP](#settings-that-affect-mcp) · [Permissions](#permissions)
<!-- /nav:top -->

Granary includes an **MCP server** (Model Context Protocol), so an AI client you already use (a desktop AI app,
an IDE assistant, an agent you run yourself) can work in the ERP with the same tools as [Nary](Nary-AI-Assistant).
A connection always acts as **one person**, with that person's permissions, narrowed further by the **level** you
choose when you connect it. This page is for any staff member who wants to connect an AI client, and for the
Owner, who can grant the highest level.

![Settings > Integrations & access > MCP access](images/mcp-access.png)

The MCP endpoint is:

```
https://<your Granary domain>/api/v1/mcp
```

It speaks JSON-RPC 2.0 over HTTP (`initialize`, `tools/list`, `tools/call`, `resources/*`, `prompts/*`).

---

## Access levels

| Level | What the AI client may do |
|---|---|
| **Read only** | Look things up. Nothing can be changed |
| **Propose** | Every change becomes an approval card; nothing changes until a person approves it in Granary |
| **Act as me** | Changes apply at once, like your own clicks. Messages to customers, invoices and meetings still wait for approval |
| **Full power (Owner)** | Only for the person who approves Nary's work (the Owner). Act as me, plus everything Nary does: decide its cards, answer its questions, pause it, send WhatsApp and email as Nary, book and change meetings, draft invoices. Messages, meetings and money go only after the person's yes in the chat |

Whatever the level, a connection can never do more than the person's own permissions allow. Everything an AI
client does is recorded as coming from MCP, with the client's name. Only staff accounts can connect AI clients;
portal users can't.

Levels that can act (**Act as me** and **Full power**) need extra care:

- you type your **password** (and, with two-factor sign-in on, your **authenticator code**) when you create or
  allow the connection;
- the connection ends after `mcp.pat.max_days_power` days at most (default 90), however often the client uses
  or refreshes it.

---

## Connect with a browser login (OAuth)

This is the easiest way. Use it when your AI client supports MCP login.

1. In your AI client, add an MCP server with only the address `https://<your domain>/api/v1/mcp`.
2. The client finds Granary's login by itself and opens your browser on **Connect an agent to Granary**. If you
   are not signed in to Granary, sign in first.
3. Check the client's name and where it will be sent back to.
4. Pick the **Access level**. The page starts on **Propose** (or **Read only**) even if the client asked for more:
   a level that can act must be your own choice.
5. For **Act as me** or **Full power (Owner)**, enter **Your password** and, if you use two-factor sign-in, the
   **Authenticator code**.
6. Click **Allow** (or **Deny**).

![The consent page an AI client opens](images/mcp-oauth-consent.png)

The client receives an access token that renews itself. Related settings: **Login token lasts**
(`mcp.oauth.access_minutes`), **Stay logged in unused for** (`mcp.oauth.refresh_days`) and **Sign-in code valid
for** (`mcp.oauth.code_minutes`).

---

## Connect with a personal access token

Use this for AI clients that can't open a browser login.

1. Open **Settings > Integrations & access > MCP access**.
2. Enter a **Token name** (for example "My AI client, laptop").
3. Pick the **Level**. The Owner also sees **Full power (Owner)**.
4. Optional: **Expires (days)**. Blank uses the default (`mcp.pat.default_days`).
5. For a level that can act, enter **Your password** and, with two-factor sign-in on, the **Authenticator code**.
6. Click **Create** and copy the token right away. **It is never shown again.**

Then configure your client to send it as a bearer header:

```json
{
  "mcpServers": {
    "granary": {
      "url": "https://granary.example.com/api/v1/mcp",
      "headers": { "Authorization": "Bearer gpat_..." }
    }
  }
}
```

---

## See and revoke connections

The table on **MCP access** lists your tokens and browser logins with their level, last use and expiry. Click
**Revoke** to end one at once. Admins can switch between **My connections** and **All connections** and revoke
anyone's.

Changing your password disconnects all your tokens and AI client connections.

---

## How changes are applied

- **Read only**: tools that change data are not offered.
- **Propose**: a change becomes a card in Granary (for example in **Nary > Approvals** or on the record). A person
  approves it there.
- **Act as me**: CRM changes and similar edits apply at once as you. Customer messages, invoices and meetings still
  become cards.
- **Full power (Owner)**: sensitive actions (a customer message, an invoice, a meeting, a discount, a tax or a
  money change, a delete or a merge) take two steps:
  1. The client prepares the action. Granary returns the **final version** and an `apply_id`. Nothing has
     happened yet; the client must show you the final version word for word.
  2. If you want it, the client calls `nary_apply` with that `apply_id`. Your AI client asks you to **Allow** that
     call, and only your click applies it. The `apply_id` is single-use and expires after **Full-power confirm
     code valid for** (`mcp.owner.confirm_minutes`).

### The authenticator code on `nary_apply`

With **Applying needs the authenticator code** (`mcp.apply.require_totp`, on by default) and two-factor sign-in on
for you, `nary_apply` on those sensitive actions also needs the current code from your authenticator app. **You**
type it in the chat; the AI must never guess or reuse one.

Why it matters: an AI can read a planted instruction in an email, a web page or a document. With the code
required, it can't send, bill, delete or merge in your name with a single careless click. If you turn the setting
off, the Allow click alone applies the action. People without two-factor sign-in are not asked for a code either
way, so **turn on two-factor sign-in for everyone who connects an AI client at full power**.

---

## Tips for AI clients

- Granary tells the client, on connect, to read the prompt `granary_assistant` (or the resource `granary://rules`)
  first: it explains how to work for the company.
- The [AI knowledge](Nary-AI-Assistant#ai-knowledge) library is used by connected clients too.
- While Nary is paused, **Nary > Work queue** shows what waits for you and your AI client, with the next tool for
  each item, and a prompt you can paste into a scheduled task in your client.

---

## Settings that affect MCP

All in **Settings > Integrations & access > MCP access**:

| Setting | What it does |
|---|---|
| Login token lasts (`mcp.oauth.access_minutes`) | Life of a browser-login access token (it renews itself) |
| Stay logged in unused for (`mcp.oauth.refresh_days`) | How long an unused browser login stays valid |
| Sign-in code valid for (`mcp.oauth.code_minutes`) | The one-time code during browser login |
| Personal token lifetime (`mcp.pat.default_days`) | Default expiry of new tokens |
| Longest life of a token that can act (`mcp.pat.max_days_power`) | Act and full-power connections end after this many days at most |
| Full-power confirm code valid for (`mcp.owner.confirm_minutes`) | How long a prepared action waits for `nary_apply` |
| Applying needs the authenticator code (`mcp.apply.require_totp`) | See above. Default on |

## Permissions

- Any staff account can create its own connections at **Read only**, **Propose** or **Act as me**.
- **Full power (Owner)** is offered only to the Owner and whoever holds Nary's approver role.
- System admins and the Owner can list and revoke everyone's connections.

## Related pages

- [Nary AI Assistant](Nary-AI-Assistant)
- [Administration](Administration#two-factor-sign-in)
- [Configuration](Configuration)

<!-- nav:bottom -->
---

← Previous: [Nary AI Assistant](Nary-AI-Assistant) | Next: [CRM](CRM) →
<!-- /nav:bottom -->
