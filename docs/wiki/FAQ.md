# FAQ

<!-- nav:top -->
[Home](Home) › Getting started › **FAQ**
<!-- /nav:top -->

Short answers to common questions. Each one links to the page with the details.

---

## General

**What does Granary cost?**
Nothing. Granary is open source under MIT OR Apache-2.0. You pay only for your server and for any outside service
you choose to use (an AI provider, an exchange-rate API).

**Can I run it in the cloud, or only on my own server?**
Anywhere you can run Docker Compose: a server in your office or a cloud virtual machine. See
[Installation](Installation).

**How big a server do I need?**
At least 2 cores, 4 GB RAM and 20 GB disk; 4 cores, 8 GB RAM and 50 GB SSD are recommended. Compiling the Rust
images needs about 8 GB RAM, or build them on another machine.

**Can I use only some modules?**
Yes. Modules you don't use stay empty. Access to each module is controlled by roles and permissions, so people see
only what they need. See [Administration](Administration#roles-and-permissions).

**Which currencies does it support?**
Supported currencies are USD, EUR, GBP, JPY, SGD, AUD and CNY. See
[Multi-Currency](Multi-Currency).

---

## Setup

**What if the Owner forgets their password?**
Only the Owner can reset the Owner's password in the app, and changing `SUPERADMIN_PASSWORD` in `.env` has no
effect once the Owner exists. Keep the Owner's password in a password manager, and save the two-factor recovery
codes.

**I changed `SUPERADMIN_EMAIL` in `.env`, but nothing happened.**
The Owner is created only on the first start, while no superadmin exists. Later changes to these values are ignored.

**Where do I put my AI key, mail passwords and other secrets?**
In **Settings**, where they are stored encrypted. Not in `.env`. See [Configuration](Configuration#secrets-in-settings).

**Why does an action fail with "Setting 'x' is not set"?**
Every business number is a setting, and Granary never guesses one. Enter the value in Settings; the message names
it. See [Troubleshooting](Troubleshooting#setting-x-is-not-set-400).

**My functional currency is USD. Why does a small purchase order go to the Owner?**
It shouldn't. The shipped approval limits are sized for a currency like JPY. Adjust every limit for your currency.
See [First Steps](First-Steps#4-approval-limits).

**Can I change the functional currency later?**
Only until the first journal entry is posted. Set it before you start. See
[First Steps](First-Steps#2-functional-currency).

**Why are there no taxes?**
A fresh install has none, because tax rules differ by country. Add yours in **Finance > Taxes**.

**Do I need to create storage buckets?**
No. Granary creates each bucket the first time it stores a file there.

---

## Everyday use

**How do I find a page or a setting quickly?**
Press **Ctrl K** (Cmd K on a Mac) and type.

**How do I open Nary?**
Click **Ask AI** in the top bar or press **Ctrl J**. See [Nary AI Assistant](Nary-AI-Assistant).

**Can Nary change data by itself?**
Not in the chat. Anything that would change data comes back as a card, and nothing is saved until a person applies
it. Nary's own work as an assistant follows the autonomy level of each action. See
[Nary AI Assistant](Nary-AI-Assistant#confirm-cards).

**Can Nary see data I can't?**
No. It runs with your permissions.

**Can I use Granary without AI?**
Yes. Switch off **AI features** in **Settings > Nary & AI > AI features**. Everything else works.

**Does WhatsApp work out of the box?**
No. Granary does not include a WhatsApp Web bridge. You need to provide one at `WHATSAPP_SERVICE_URL`. Without it,
the WhatsApp features show "not connected".

**Can I connect my own AI app?**
Yes, over MCP, as yourself and at the level you choose. See [MCP for AI Clients](MCP-for-AI-Clients).

**Video calls fail between offices.**
Check the TURN server: `TURN_EXTERNAL_IP`, `TURN_URIS` and the firewall ports. See
[Installation](Installation#turn-server-for-video-calls-coturn).

---

## Security

**Should I turn on two-factor sign-in?**
Yes, at least for the Owner, every system admin and everyone who connects an AI client. See
[Administration](Administration#two-factor-sign-in).

**How do I sign someone out of every device?**
Reset their password in **Settings > Users**: it ends all their sessions. Changing your own password ends every
other session of yours and disconnects your AI client connections.

**Granary refuses to start and mentions a placeholder.**
`JWT_SECRET_KEY`, `APP_SECRET_KEY` or `NEXTAUTH_SECRET` still holds the example value. Generate real values. See
[Troubleshooting](Troubleshooting#the-backend-exits-at-start-because-of-a-placeholder-secret).

**How do I report a security problem?**
Privately, through the repository's **Security** tab (**Report a vulnerability**), never in a public issue.

---

## Maintenance

**How do I back up?**
Back up the database, the `storage_data` and `signing_keys` volumes, and your `.env`. See
[Backups and Upgrades](Backups-and-Upgrades).

**How do I upgrade?**
Back up, check out the new release tag, set `GRANARY_VERSION` to it, then `docker compose pull` and
`docker compose up -d`. Migrations run on start. See
[Backups and Upgrades](Backups-and-Upgrades#upgrade-granary).

**Where are the logs?**
`docker compose logs -f backend` (and `worker`, `beat`, `frontend`). Nginx logs are in `/var/log/nginx/`.

## Related pages

- [Home](Home)
- [Troubleshooting](Troubleshooting)

<!-- nav:bottom -->
---

← Previous: [First Steps](First-Steps) | Next: [AI Across Granary](AI-Across-Granary) →
<!-- /nav:bottom -->
