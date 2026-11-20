# First Steps

<!-- nav:top -->
[Home](Home) › Getting started › **First Steps**

**On this page:** [1. Company details and regional settings](#1-company-details-and-regional-settings) · [2. Functional currency](#2-functional-currency) · [3. Taxes](#3-taxes) · [4. Approval limits](#4-approval-limits) · [5. Users and roles](#5-users-and-roles) · [6. Mail: system email and CRM inboxes](#6-mail-system-email-and-crm-inboxes) · [7. AI provider key](#7-ai-provider-key) · [8. Other things to check](#8-other-things-to-check)
<!-- /nav:top -->

You have installed Granary and signed in as the Owner. This page lists what to set before you invite the team or
send the first document. Work through it in order: later steps depend on earlier ones (approval limits are in
the functional currency, for example). Most of it happens in **Settings** (sidebar **Settings > General**).
Only the Owner (superadmin) and system admins can change system settings.

![Settings, Company & branding area](images/first-steps-settings-company.png)

The examples use a fictional company, **Kumo Trading Co.**, a small office-equipment trading firm in Tokyo that
keeps its books in JPY and has some USD and EUR customers.

> Tip: the search box at the top of Settings finds any setting by name or key. Press **Ctrl K** (Cmd K on a Mac)
> anywhere to jump to a page or a setting.

---

## 1. Company details and regional settings

Open **Settings > Company & branding**.

1. **Company identity**: legal name, trade name, tax ID, registration number, address, signatory name and
   title. These appear on quotations, invoices and letters.
2. **Document numbers**: the prefixes for quotations, invoices and proformas (`QUO`, `INV`, `PRO` by default) and
   the default payment terms.
3. **Regional**: set the **time zone** first (for Kumo Trading, `Asia/Tokyo`). It stays `UTC` until you change
   it. Document numbers, default dates, overdue checks, exchange-rate lookups, payroll and daily limits all use
   this company time zone. Also set the date format.
4. **App logo**, **Favicon**, **Login page text** and **Letterhead** (the header image on printed documents).
5. Click **Save** in the save bar.

---

## 2. Functional currency

Open **Settings > Money & tax > Currencies**.

1. Pick the **Functional currency**: the base currency for the ledger, the profit and loss statement, the
   balance sheet and tax reports. It defaults to USD. For Kumo Trading, choose **JPY**.
2. Tick the **Active currencies** your customers and vendors use (for example JPY, USD, EUR). Only these appear
   in forms. Supported: USD, EUR, GBP, JPY, SGD, AUD, CNY.
3. Optional: paste an **ExchangeRate API key** (a free key from exchangerate-api.com), click **Test**, then
   **Sync now**. Rates then sync every 6 hours. Without a key, enter rates by hand in **Finance > Exchange Rates**.

> **Set this before anything is posted.** The functional currency can't be changed once journal entries exist.
> Granary never treats a missing exchange rate as 1.0: a foreign-currency document with no rate for its date is
> refused. See [Multi-Currency](Multi-Currency).

Money is rounded to each currency's minor unit (`currency.decimals` in **Money & tax > Accounting**: JPY 0, the
others 2). If you activate a currency, check it has an entry there.

---

## 3. Taxes

A fresh install has **no taxes**. Open **Finance > Taxes** and click **New tax** for each tax you charge or
withhold: **Tax code**, **Tax name**, **Tax type**, **Rate (%)**, whether it **Applies to sales** and/or
**Applies to purchases**, and the tax accounts.

Example for Kumo Trading: `CT10`, consumption tax 10%, applies to sales and purchases.

Then, in **Settings > Money & tax > Accounting**, set **default sales tax** per currency (`finance.default_sales_tax`,
for example `{"JPY": "CT10"}`). It is preselected on every line when a quotation becomes an invoice.

Payroll income tax is not in this table. It is configured under **Settings > People & payroll** (see
[HR and Payroll](HR-and-Payroll)).

---

## 4. Approval limits

Open **Settings > Approvals & signing > Approval policies**.

Granary ships one approval policy per document type: quotation, proposal, contract, invoice, purchase order,
bill of materials, contractor request, work order, vendor invoice, expense, marketing content, hiring request,
offer letter, SOP and payroll run. Each policy is an **Approval chain** of steps, and steps can carry conditions
such as "amount above N".

> **Amounts are in the functional currency, and the shipped limits are sized for a currency like JPY.** For
> example, a purchase order above 5,000,000 goes to the Owner, and an expense above 2,000,000 per claim (or
> 1,500,000 per item) goes to the Owner too. If your functional currency is USD or EUR, these numbers are far
> too high. **Review every limit before anyone submits a document.**

For each policy:

1. Open it and check each step's approver and condition.
2. Adjust the amounts to sensible values in your currency.
3. Check the **Discount allowances** on the quotation policy (discounts within an allowance don't need the
   Owner's approval).
4. Save. Changes apply to new submissions; requests already in progress keep their chain.

Rules worth knowing:

- Quotations are approved automatically only when every line is a catalog item at list price with no discount.
- The Owner approves every invoice and credit note by default.
- When an amount can't be evaluated (a purchase order with no exchange rate, for example), every amount-guarded
  step applies.

See [Approvals and Signing](Approvals-and-Signing).

---

## 5. Users and roles

1. Open **Settings > Users** and click **Add user** for each team member: **Full name**, **Email**,
   **Password**, **Portal type** (**Internal (staff)** for employees), **Department**, **Position**.
2. Link each staff user to their **employee record**. The link connects the sign-in to attendance, leave, payroll
   and time logs.
3. Give each person a role. Granary seeds a set of roles (see
   [Administration](Administration#roles-and-permissions)).
4. Tick **System administrator** only for people who should manage all settings.

Partners and vendors get portal accounts instead (see [Administration](Administration#portal-users)).

---

## 6. Mail: system email and CRM inboxes

Open **Settings > Email & WhatsApp**.

1. **Test recipient**: an address you can read. The **Send test** buttons send there.
2. **System email**: the SMTP account for notifications, password resets and approval emails (for example
   `erp@example.com`).
3. **CRM inboxes**: click **Add inbox** for each shared mailbox (for example `sales@example.com`), with its SMTP
   and IMAP servers. Test it. One inbox is the **Default**. Quotations, invoices and email campaigns are sent
   through a CRM inbox, and replies thread back into the CRM.
4. **Email signature**: the layout all staff signatures use. Each person fills in their own fields in their
   profile.

Passwords are stored encrypted in the database, never in `.env`. See [CRM Inboxes](CRM-Inboxes).

---

## 7. AI provider key

Open **Settings > Nary & AI**.

1. **Providers & keys**: paste the API key of at least one provider (Anthropic, OpenAI or an OpenAI-compatible
   endpoint, Gemini, DeepSeek; Ollama needs no key). Click **Save**, then **Test**. Keys are stored encrypted.
2. **Model per job**: the shipped routing uses Anthropic models for most jobs and Gemini for embeddings and web
   search. **Route every job to a provider you have a key for.** Embeddings need a provider with an embeddings
   API.
3. **Budget**: set the monthly AI budget (default 100 USD, 0 means no cap).
4. **Web research**: pick **SearXNG (self-hosted, free)** to enable CRM enrichment and research.

To run Granary without AI, switch off **AI features** in **Nary & AI > AI features**. See
[Nary AI Assistant](Nary-AI-Assistant).

---

## 8. Other things to check

| What | Where |
|---|---|
| Payroll contributions, income tax and overtime (payroll refuses to run while they are empty or malformed) | **Settings > People & payroll** |
| Office locations for face clock-in | **Settings > People & payroll > Office locations** |
| Account mapping before you turn on automatic ledger posting | **Finance > Chart of Accounts** (see [Finance](Finance)) |
| Fiscal periods for the next year (the seed creates only the install year) | **Finance > Fiscal Periods** |
| Pricing rules: the defaults refuse a quotation line priced below 1.15 times its full cost or under a 10% margin; set them for your business before the first quotation | **Settings > Sales & CRM > Pricing rules** |
| Two-factor sign-in for yourself and every admin | **My profile**, **Two-factor sign-in** card |
| Placeholder secrets: when the API starts, it alerts the Owner about any weak secret in `.env` or Settings | Your notifications |

---

## Related pages

- [Installation](Installation)
- [Configuration](Configuration)
- [Administration](Administration)
- [Multi-Currency](Multi-Currency)
- [Approvals and Signing](Approvals-and-Signing)

<!-- nav:bottom -->
---

← Previous: [Installation](Installation) | Next: [FAQ](FAQ) →
<!-- /nav:bottom -->
