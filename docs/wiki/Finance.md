# Finance

<!-- nav:top -->
[Home](Home) › Using Granary › Money › **Finance**

**On this page:** [AI in this module](#ai-in-this-module) · [Find your way around](#find-your-way-around) · [Invoices](#invoices) · [Payment schedules](#payment-schedules) · [Expenses](#expenses) · [Chart of accounts](#chart-of-accounts) · [Journal entries and automatic posting](#journal-entries-and-automatic-posting) · [Fiscal periods](#fiscal-periods) · [Bank reconciliation](#bank-reconciliation) · [Taxes](#taxes) · [Reports](#reports) · [Fixed assets](#fixed-assets) · [Budgets](#budgets) · [Settings that affect this module](#settings-that-affect-this-module) · [Permissions](#permissions)
<!-- /nav:top -->

The Finance module is where you bill customers, collect and refund money, record expenses and keep the books.
It covers invoices and proformas (with a discount and a tax on each line), credit notes, refunds, payments,
payment schedules, expense claims, the chart of accounts, journal entries with automatic posting from
documents, fiscal periods, bank reconciliation, taxes, financial reports, fixed assets and budgets. Finance
staff use it every day; managers use the reports, the approval steps and the budgets.

![The invoice list with saved views and the outstanding, paid and overdue tiles](images/finance-invoices-list.png)

---

## AI in this module

Nary can answer finance questions, and it prepares invoice drafts as cards that a person creates. See
[AI Across Granary](AI-Across-Granary).

- **Ask Nary**: "Which invoices are overdue?" or "How much have we invoiced and collected?" (needs
  `finance:view_invoices` or `finance:view_reports`).
- **Invoice drafts from the chat**: "Make a proforma for Hoshino Design Office for the 12 chairs". Nary shows a
  draft invoice card (a proforma by default) with catalog lines priced by the pricing service. Click **Create**
  (needs `finance:create_invoice`).
- **Down-payment proforma**: after a contract draft, Nary proposes the proforma for the quotation's first
  milestone as a card.
- **Deal won on payment**: when Nary runs on its own, recording the down payment on a deal whose contract is signed
  moves the deal to **Won** (`nary.contract.won_needs_dp`).

Nary has no tools for the ledger, bank, expenses, taxes or financial reports beyond the revenue summary.

---

## Find your way around

Open **Finance** in the sidebar. It has these pages:

| Page | What it is for |
|---|---|
| **Invoices** | Commercial invoices, proformas and credit notes |
| **Payments** | Every payment recorded against an invoice |
| **Expenses** | Expense claims, their approval and reimbursement |
| **Payment Schedules** | Milestones that split a quotation into phased payments |
| **Chart of Accounts** | Accounts, and the account mapping that automatic posting uses |
| **Journal Entries** | Manual and automatic journal entries |
| **Fiscal Periods** | Monthly periods and their locks |
| **Bank Reconciliation** | Bank accounts, statements and matching |
| **Taxes** | Sales taxes and withholding taxes |
| **Exchange Rates** | FX rates (see [Multi-Currency](Multi-Currency)) |
| **Reports** | P&L, balance sheet, trial balance, AR aging, general ledger |
| **Fixed Assets** | The asset register, depreciation and disposals |
| **Asset Categories** | Default depreciation rules per type of asset |
| **Budget** | Budgets per year and department, compared with the ledger |

Each list page has saved view tabs, a search box and a details panel that opens when you click a row.

---

## Invoices

Granary has three kinds of invoice:

- A **commercial** invoice is the real bill. Once sent, it posts to the ledger and can't be edited. To change
  it, you issue a credit note.
- A **proforma** is a payment request. It never posts to the ledger. A payment on a proforma is a customer
  advance. You convert a proforma to a commercial invoice when the sale is final.
- A **credit note** reverses a commercial invoice.

The list tabs are **All**, **Drafts**, **Sent**, **Partly paid**, **Overdue** and **Paid**. The tiles at the top
show **Outstanding**, **Paid this month**, **Overdue** and **Draft**.

### Create an invoice

![The new invoice form with line items, per-line discount and tax](images/finance-invoice-form.png)

1. Open **Finance > Invoices** and click **New invoice**.
2. In **Client**, pick the organization and the **Contact person**. Optional: a **Billing email** and a
   **Billing WhatsApp** number. When set, they are used first when the invoice is sent; leave them empty to use
   the contact's details.
3. In **Invoice details**, choose the **Type** (**Commercial** or **Proforma**), the **Payment terms** (**Due
   immediately**, **Net 7**, **Net 14**, **Net 30**, **Net 45** or **Net 60**), the **Issue date** and the **Due
   date**.
4. Choose the **Currency**. For a currency other than your functional currency, a **Rate** field appears
   (1 unit of the invoice currency = ? functional currency). Type the rate or click **Fetch the spot rate**.
   See [Multi-Currency](Multi-Currency).
5. Add the **Line items**. For each line:
   - optional: pick a **Catalog product**, or leave it as a custom line;
   - **Item name** and an optional description;
   - **Qty**, **Unit price** and **Disc %** (a discount on that line);
   - **Tax**: one of your configured taxes, **No tax**, or **Custom %**.
6. Use the **Prices exclude tax** switch to say whether the unit prices are before or after tax.
7. Add **Notes** (payment instructions, bank details).
8. Click **Create invoice**.

The **Summary** card shows the subtotal, discount, tax and total as you type. If no sales tax is set up yet, a
**No tax rates set up** box links to **Finance > Taxes**.

A line keeps a copy of the tax code and rate it used, so a later change to a tax rate never rewrites an issued
invoice.

Rules the server applies:

- Only a draft invoice can be edited. Click **Edit** on the invoice page.
- The currency and the type can't change after the invoice is created. Delete the draft and create a new one.
- An invoice must keep at least one line.
- Only a draft invoice can be deleted.
- A foreign-currency invoice needs an exchange rate: the one you type, or a rate on record for the issue
  date. Without either, saving fails with "No USD/JPY exchange rate is on record for ...".

Invoices are also created from an accepted quotation; see [Sales](Sales).

### Send an invoice

Nothing leaves Granary before it is approved.

1. Open the invoice and click **Send invoice**.
2. If the invoice is not approved yet, Granary submits it for approval and tells you it now waits. It is
   approved at once when it is within your standing authority. The approval panel on the invoice page shows
   who must approve.
3. Once it is approved, click **Send invoice** again. The send dialog opens with the recipient filled in.
4. Choose email (**From**, **To**, **Cc**, **Bcc**, **Greeting name**) or WhatsApp (**WhatsApp number or
   chat**), add an optional message, then click **Confirm & send**. The approved PDF with its verification QR
   is attached.

To send again, click **Resend** on a sent, partly paid or overdue invoice. Click **PDF** to download it. See
[Approvals and Signing](Approvals-and-Signing) for approval policies.

![Sending an invoice: submit for approval, approve, then send by email](images/finance-invoice-send.gif)

### Record a payment

1. Open the invoice and click **Record payment** (on a sent, partly paid or overdue invoice).
2. Enter the **Amount** (it starts at the amount due) and the **Payment date**.
3. For a foreign-currency invoice, enter the **Exchange rate** on the payment date, or leave it blank to use
   the rate on record.
4. Choose the **Method**, the bank account it was **Received into**, a **Reference** and optional **Notes**.
5. Click **Record payment**.

The payment must be in the invoice's currency and can't be more than the amount due. If the rate on the
payment date differs from the invoice's rate, Granary books the difference as an FX gain or loss (see
[Multi-Currency](Multi-Currency)). A payment can't be recorded on a credit note.

**Finance > Payments** lists every payment with its client, date, amount, method, reference and invoice. The
tiles show **Total collected** and **This month**. Payments are recorded only from invoice pages.

### Revise a proforma

A sent proforma can be revised:

1. Open the proforma and click **Revise**.
2. Enter the **Reason for revision** and click **Create revision**.

The current proforma becomes **superseded** (read-only) and a draft with the next revision number opens for
editing. The **Revisions** card lists every revision. Drafts are edited directly; only a sent proforma is
revised.

### Convert a proforma to a commercial invoice

Click **Convert to commercial** on a draft or sent proforma. The new commercial invoice carries the balance.
The proforma becomes **converted** and read-only, with a link to the new invoice. A proforma can be converted
only once. A sent proforma that is still unpaid can also be cancelled.

### Issue a credit note

Commercial invoices can't be edited after they are sent. To correct or cancel one:

1. Open the commercial invoice and click **Issue credit note**.
2. Enter the **Reason for credit note** and click **Issue credit note**.

Granary creates a draft credit note that mirrors the invoice (its lines can't be changed). Send it like an
invoice: it goes through approval first. When the credit note is sent, the original invoice is cancelled.
An invoice can have only one credit note at a time.

### Record a refund

If the customer had already paid an invoice that a credit note cancelled, the invoice shows a **Refund due**
banner.

1. Click **Record refund**.
2. Enter the amount (at most the refund due), **Paid on**, **Paid from** (the bank account) and a
   **Reference**.
3. Save. The **Refunds** card on the invoice lists it.

### Overdue reminders

Granary emails the billing contact of every overdue, unpaid invoice on the days set in **Overdue reminders**
(for example 1, 7, 14 and 30 days after the due date), then every **Then remind every** days after the last
one. See [Settings that affect this module](#settings-that-affect-this-module).

---

## Payment schedules

A payment schedule splits a quotation into milestones, for example a deposit, a progress payment and a final
payment.

1. Open **Finance > Payment Schedules** and click **Add milestone**.
2. Pick the **Quotation**, then enter the **Milestone name**, the **Amount** and the **Due date**.
3. Click **Add milestone**.

The list shows each milestone's quotation, sequence, share, due trigger, due date, amount and whether it has
been invoiced. Overdue milestones are tagged; the **Due within 14 days** chip shows the ones coming up.

The server refuses milestones whose shares add up to more than 100% of the quotation, and amounts in a
currency other than the quotation's. Only pending milestones can be deleted. Nary can raise the proforma for
a milestone when you approve its card (see [Nary AI Assistant](Nary-AI-Assistant)); a milestone can be
invoiced only once.

---

## Expenses

Anyone with the right to submit expenses can claim money back.

### Submit an expense

1. Open **Finance > Expenses** and click **Submit expense**.
2. Enter a **Description**, the **Amount**, the **Date** and the **Currency**.
3. For a foreign currency, enter the **Rate** or click **Fetch the rate**. The amount in your functional
   currency is shown under it.
4. Pick a **Category**: Travel, Meals, Office Supplies, Software, Hardware, Marketing, Utilities,
   Professional Services or Other.
5. If your manager approved the expense before you spent the money, tick the pre-approval box. Your expense
   policy may require it above a limit; without it, a claim above the limit is refused with the policy's
   message.
6. Click **Submit expense**.

The claim goes to its approval chain at once.

### Review and reimburse

- The **Mine** and **Needs my review** tabs filter the list. Without the approve permission you see only your
  own claims.
- Click **Approve**, or **Reject** and give a **Reason for rejection**.
- When the money is paid back, click **Mark reimbursed**, choose **Paid from** (the bank account) and the
  **Paid date**. Only an approved expense can be reimbursed.
- Only pending or rejected expenses can be deleted.

An expense posts to the ledger when it is approved and again when it is reimbursed. Its category decides the
expense account (see [Account mapping](#account-mapping)).

---

## Chart of accounts

Open **Finance > Chart of Accounts**. The **Accounts** tab lists every account with its code, category, type,
normal balance, currency, direct posting flag and status.

To add an account:

1. Click **Add account**.
2. Enter the **Account code** (for example 1300), the **Category**, the **Account name**, the **Account
   type**, the **Currency** and a **Description**.
3. Turn on **Allow direct posting** if people may post manual journal entries to it. Leave it off for an
   account that only its documents feed (for example accounts receivable).
4. Click **Create**.

Account codes must be unique. An account that has journal lines can't be deleted: deactivate it instead.

### Account mapping

Automatic posting doesn't name accounts directly. Each posting line names a **role** (for example `ar`,
`revenue`, `sales_tax_output`, `expense.travel`) and the **Account mapping** tab says which account each role
uses.

1. Open **Finance > Chart of Accounts** and click **Account mapping**.
2. Pick an account for each role. Roles with no account show **Not mapped**.
3. To give an expense category its own account, type it in **New expense category** (for example `travel`)
   and map the new `expense.<category>` role.
4. Save.

An unmapped role blocks posting for the document that needs it, with a message naming the role. An
`expense.<category>` role with no account falls back to `expense_default`. A fresh install maps every role to
the default chart. The full role list is in [Configuration](Configuration).

---

## Journal entries and automatic posting

### Automatic posting

From the date in **Post to the ledger automatically from**, documents post their own journal entries:

| Document | Posts when |
|---|---|
| Commercial invoice | It is sent |
| Payment | It is recorded (reversed when it is deleted) |
| Credit note | It is issued |
| Refund | It is paid |
| Vendor invoice | It is approved, and when it is paid |
| Expense | It is approved, and when it is reimbursed |
| Payroll | It is paid |
| Asset depreciation and disposal | They are posted |

Proformas never post. Each event posts once. A payment, vendor payment, payroll payout, reimbursement or
refund that names a bank account posts to that bank account's GL account; without one it uses the
`bank_default` role.

Leave the setting blank to keep automatic posting off. Before you turn it on, map the accounts, link your bank
accounts to GL accounts, and bring older documents in with the one-time opening migration: the **Migrate
opening entries** button on **Finance > Bank Reconciliation** creates entries for documents dated before the
cut-over. See [Configuration](Configuration) for the full checklist.

An invoice, payment or payroll page shows a link to its journal entry once it has posted.

### Write a manual entry

![A new journal entry with balanced debit and credit lines](images/finance-journal-new.png)

1. Open **Finance > Journal Entries** and click **New entry**.
2. Enter the **Date**, the **Narration** and an optional **Reference**.
3. Choose the **Currency**. For a foreign currency, enter the **Rate** or click **Fetch the rate**.
4. For each line, pick the **Account** and enter a **Description** and either a **Debit** or a **Credit**.
   Click **Add line** for more.
5. Click **Save as draft**.

The entry needs at least two lines, each with a debit or a credit above 0 (not both), and the debits must
equal the credits exactly. **Total debit** and **Total credit** show under the lines. Every account must be
active and allow direct posting.

### Post, unpost, reverse

- **Post** a draft to put it in the ledger.
- **Unpost** a posted entry to return it to draft.
- **Reverse** a posted entry to create an offsetting entry dated today. The original stays posted and is shown
  as reversed. A reversed entry, or a reversal itself, can't be unposted or reversed again.
- Only draft entries can be deleted.

The tabs are **All entries**, **Drafts**, **Posted** and **Reversed**.

---

## Fiscal periods

Open **Finance > Fiscal Periods**. Each month of the chosen year shows as a card.

- Use **Previous year** and **Next year** to move between years.
- Click **Create *year* periods** to create the twelve periods of a year. A fresh install has only the periods
  of the year it was installed.
- Click **Lock** to close a month. A locked period accepts no new or changed journal entries: posting into it,
  automatic or manual, fails with "Fiscal period ... is locked". Click **Unlock** to open it again.

---

## Bank reconciliation

![Bank reconciliation: a statement's lines matched to journal lines](images/finance-bank-reconciliation.png)

### Add a bank account

1. Open **Finance > Bank Reconciliation** and click **Add bank account**.
2. Enter the **Account name**, the **Bank name**, the **Currency** and the **Account number**.
3. Pick the **GL account** from the chart of accounts. Payments that name this bank post to it, and matching
   needs it.
4. Optional: **Notes**. Turn off **Active** for an account you no longer use (it stays for history but is not
   offered for new payments).
5. Click **Create bank account**.

The currency of a bank account can't change once payments or statements are recorded on it.

### Reconcile a statement

1. Select the bank account, then click **New statement**.
2. Enter the **Period start**, **Period end**, **Opening balance** and **Closing balance**, then click
   **Create statement**.
3. Click **Add lines** and paste the rows from your bank's export (date, description, reference, debit, credit,
   balance). Choose the **Number format** (decimal comma or decimal point). Turn on **CR = money in** if your
   bank's debit column is money out. Click **Preview**, fix any rows marked in error, then click **Import**.
4. For each line, click **Match** and pick the posted journal line on the bank's GL account with the same
   amount. Click **Exclude** for a line with no ledger counterpart (it still counts toward the closing
   balance). **Unmatch** and **Excluded, undo** reverse these.
5. Click **Mark reconciled**.

Granary refuses to reconcile while any line is neither matched nor excluded, or when opening balance + money
in - money out doesn't equal the closing balance (a line is missing, duplicated or misread). A reconciled
statement is locked: you can't add or change its lines.

![Matching statement lines and marking the statement reconciled](images/finance-bank-match.gif)

---

## Taxes

Sales and purchase taxes live only in **Finance > Taxes**. A fresh install has none.

1. Click **Add tax**.
2. Enter the **Tax code** (for example `VAT10`), the **Tax type** (Sales tax / VAT, Withholding: services,
   Withholding: employees, Income Tax, Stamp Duty or Other) and the **Tax name**.
3. Optional: a **Law reference**.
4. Enter the **Rate (%)** and the **Rate base** (% of invoice subtotal, % of item price, or a custom taxable
   base).
5. Tick **Applies to sales** and/or **Applies to purchases**.
6. Pick the **Tax payable account** and the **Tax receivable account**, add **Notes**, and click **Create**.

Tax codes must be unique. The tabs are **All**, **Sales**, **Purchases** and **Inactive**.

When a quotation becomes an invoice, each line gets the tax in **Default sales tax by currency** for the
invoice's currency (none by default). You can change it per line on the draft.

Withholding tax on vendor invoices and work orders is described in [Procurement](Procurement). Payroll income
tax doesn't use this table; see [HR and Payroll](HR-and-Payroll).

---

## Reports

![The profit and loss report for a period](images/finance-reports-pl.png)

Open **Finance > Reports**. The tabs are:

| Report | What you choose | What it shows |
|---|---|---|
| **P&L** | **From**, **To** | Revenue and expense accounts, total revenue, total expenses and net income |
| **Balance sheet** | **As of date** | Assets, liabilities and equity, with a liabilities + equity check |
| **Trial balance** | **As of date** | Debits and credits per account, marked **Balanced** or **Out of balance** |
| **AR aging** | **As of date**, optional **Include proformas (collections view)** | Outstanding invoices in Current, 1-30, 31-60, 61-90 and 90+ days buckets |
| **General ledger** | **Account**, **From**, **To** | Every posted line on one account, with the closing balance |

Reports are in the functional currency and come from posted journal entries only. AR aging counts commercial
invoices by default; tick the proformas box to see everything you are waiting to collect.

---

## Fixed assets

### Set up asset categories

1. Open **Finance > Asset Categories** and click **New category**.
2. Enter the **Category name** (for example Vehicles, IT Equipment, Furniture), a **Description**, the
   **Depreciation method** (Straight-line, Declining balance (DDB), or None (manual)), the **Useful life
   (months)** and the **Salvage value %**.
3. Click **Create category**.

A category also holds the asset, depreciation and accumulated depreciation accounts; the list shows them
under **Asset account**, **Depreciation account** and **Accumulated account**. Posting depreciation, and
recording a disposal, are refused until these accounts are set on the asset's category.

### Register an asset

1. Open **Finance > Fixed Assets** and click **Add asset**.
2. Enter the **Asset name**, pick the **Category**, and enter the **Currency**, **Purchase date** and **Purchase
   price**.
3. Optional: override the **Depreciation method**, **Useful life (months)** and **Salvage value** (blank uses
   the category default), and add the **Location** and **Serial number**.
4. Click **Create asset**.

The salvage value can't exceed the purchase price.

### Depreciate and dispose

- Click **Depreciation schedule**, then **Generate / refresh** to build the timeline, and **Post next period**
  to post the next pending period.
- Click **Record disposal**, enter the **Disposal date**, the **Disposal type** (Sold, Scrapped or Donated), the
  proceeds, **Proceeds received into** (the bank account) and **Notes**. A disposed asset is no longer
  depreciated.

The tabs are **All**, **Active**, **Fully depreciated** and **Disposed**.

---

## Budgets

Budgets are kept in the functional currency and compared with the ledger.

1. Open **Finance > Budget** and click **New budget**.
2. Enter the **Budget name** (for example "Sales Dept 2027"), the **Fiscal year**, the **Period type**
   (Annual, Quarterly or Monthly), the **Department** and **Notes**. Click **Create budget**.
3. Open the budget and click **Add line**. Pick the **Account**, add a **Label**, the **Amount** and,
   for a monthly or quarterly budget, the **Period** (leave it blank for the full period). Click **Add line**.
4. Click **Approve** when the budget is final.

Each budget row shows **Total budget**, **Total actual** and **Variance**, and every line is marked **On track**
or **Over**. The tabs are **All**, **Drafts**, **Approved**, **Active** and **Closed**.

The lines of an approved budget are read-only. Only draft budgets can be deleted.

---

## Settings that affect this module

Open **Settings > General** and use the search box, or go to the area named below.

| Setting | Key | Where | What it does |
|---|---|---|---|
| Post to the ledger automatically from | `finance.gl_autopost_from` | Money & tax > Accounting | The cut-over date for automatic posting. Blank is off |
| Decimal places per currency | `currency.decimals` | Money & tax > Accounting | Rounding of every amount (for example JPY 0, USD 2) |
| Default sales tax by currency | `finance.default_sales_tax` | Money & tax > Accounting | The tax code put on each line when a quotation becomes an invoice |
| Invoice due after | `invoice_due_days` | Money & tax > Invoices | Default days until an invoice is due |
| Overdue reminders | `finance.invoice_reminder_days` | Money & tax > Invoices | Days after the due date when reminders go out |
| Then remind every | `finance.invoice_reminder_repeat_days` | Money & tax > Invoices | Days between reminders after the list (0 stops) |
| Invoice prefix, Proforma prefix | `invoice_prefix`, `proforma_prefix` | Company & branding > Document numbers | Document number prefixes |
| Default payment terms | `payment_terms_default` | Company & branding > Document numbers | Days until a new invoice is due |
| Currencies | `currency.functional`, `currency.active`, `currency.api_key` | Money & tax > Currencies | See [Multi-Currency](Multi-Currency) |

Approval chains for invoices, expenses and other documents are set under **Approvals & signing**; see
[Approvals and Signing](Approvals-and-Signing).

---

## Permissions

| Permission | Allows |
|---|---|
| `finance:view_invoices` | See invoices and payment schedules |
| `finance:create_invoice` | Create, edit and delete draft invoices, revise proformas, issue credit notes, convert proformas, add milestones |
| `finance:edit_invoice` | Edit draft invoices |
| `finance:send_invoice` | Send invoices |
| `finance:mark_paid` | Mark an invoice paid |
| `finance:confirm_payment` | Record and delete payments, record refunds, mark expenses reimbursed |
| `finance:view_payments` | See the payment list |
| `finance:submit_expense` | Submit expense claims and delete your pending or rejected ones |
| `finance:view_expenses` | See expenses (only your own, without the approve permission) |
| `finance:approve_expense` | See every expense and decide approval steps |
| `finance:manage_accounts` | Add, change and delete accounts; edit the account mapping |
| `finance:view_journal` | See journal entries |
| `finance:post_journal` | Create, post, unpost, reverse and delete entries; create and lock fiscal periods; run the opening migration |
| `finance:view_reports` | See the reports (and the journal and fiscal periods) |
| `finance:manage_bank` | Bank accounts, statements, matching and reconciliation |
| `finance:manage_tax_config` | Add, change and delete taxes (`system:manage_tax` may also add and change them) |
| `finance:manage_fx` | Add and delete exchange rates |
| `finance:view_assets`, `finance:manage_assets` | See, or manage, fixed assets and categories |
| `finance:view_budget`, `finance:manage_budget` | See, or manage and approve, budgets |

Finance is for internal users only: portal users (partners and vendors) never reach it. Superadmins can do
everything. Sending or approving a document also depends on the approval policy for it.

---

## Related pages

- [Multi-Currency](Multi-Currency): functional currency, exchange rates, FX gain and loss
- [Sales](Sales): quotations that become invoices
- [Approvals and Signing](Approvals-and-Signing): approval policies and the verification QR on PDFs
- [Procurement](Procurement): vendor invoices, withholding tax and purchase orders
- [HR and Payroll](HR-and-Payroll): payroll postings
- [Configuration](Configuration): the GL role list and the checklist before turning on automatic posting

<!-- nav:bottom -->
---

← Previous: [Approvals and Signing](Approvals-and-Signing) | Next: [Multi-Currency](Multi-Currency) →
<!-- /nav:bottom -->
