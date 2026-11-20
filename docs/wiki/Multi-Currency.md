# Multi-Currency

<!-- nav:top -->
[Home](Home) › Using Granary › Money › **Multi-Currency**

**On this page:** [AI in this module](#ai-in-this-module) · [Supported currencies](#supported-currencies) · [Set the functional currency and the active currencies](#set-the-functional-currency-and-the-active-currencies) · [Get rates automatically](#get-rates-automatically) · [Enter rates by hand](#enter-rates-by-hand) · [Which rate a document uses](#which-rate-a-document-uses) · [FX gain and loss](#fx-gain-and-loss) · [Rules worth knowing](#rules-worth-knowing) · [Settings that affect this module](#settings-that-affect-this-module) · [Permissions](#permissions)
<!-- /nav:top -->

Granary lets you invoice, buy, pay and claim expenses in several currencies while keeping one set of books.
Every amount is converted to your **functional currency** (the currency of your ledger and reports) with an
exchange rate that is either fetched from ExchangeRate-API or entered by hand. Differences between the rate
on an invoice and the rate on the day it is paid are booked as FX gain or loss. A missing rate is always
refused: Granary never treats one currency as worth 1.0 of another. Administrators set the currencies once;
finance staff keep the rates up to date.

![The Currencies settings card with the functional currency, active currencies and the API key](images/multi-currency-settings.png)

---

## AI in this module

Nary never converts a price itself. Quotation drafts and price answers that Nary prepares are priced by the
pricing service in the customer's currency, from the catalog's price rows, exactly as the quotation form does.
Nary has no tools for exchange rates or FX gain and loss. See [AI Across Granary](AI-Across-Granary).

---

## Supported currencies

Granary supports seven currencies:

| Code | Currency |
|---|---|
| USD | US dollar |
| EUR | Euro |
| GBP | Pound sterling |
| JPY | Japanese yen |
| SGD | Singapore dollar |
| AUD | Australian dollar |
| CNY | Chinese yuan |

Any other code is refused when you save the currency settings.

---

## Set the functional currency and the active currencies

Only administrators can change these.

1. Open **Settings > General**, then **Money & tax > Currencies**.
2. Choose the **Functional currency**, the base currency for the P&L, the balance sheet and tax reports. A
   fresh install uses USD. A company like Kumo Trading Co. in Tokyo picks JPY.
3. Tick the **Active currencies**. Only these appear in the currency lists on invoices, expenses, journal
   entries and other forms. The functional currency is always active and is marked **(base)**.
4. Save with the save bar at the bottom of the page.

**Choose the functional currency before you post anything.** Once journal entries are posted, the functional
currency can't be changed: every posted amount is in the old currency, so a change would need a new set of
books. Granary refuses the change with an error that says so.

### Decimal places

Amounts are rounded to each currency's minor unit, set in **Decimal places per currency** (**Money & tax >
Accounting**, key `currency.decimals`). The default is 0 for JPY and 2 for the others. A currency that is
missing from this table makes the action fail with an error naming the setting.

---

## Get rates automatically

Granary can fetch rates from ExchangeRate-API (a free key is enough).

1. Get a key at exchangerate-api.com.
2. Open **Settings > General**, then **Money & tax > Currencies**.
3. Paste the key in **ExchangeRate API key** and save. The key is stored encrypted; afterwards the field only
   says it is saved. Paste a new key to replace it.
4. Click **Test** to check the connection.
5. Under **Exchange rate sync**, click **Sync now**. Rates update within a few seconds.

After that, rates sync automatically every 6 hours. Each sync stores today's rate for every pair of active
currencies, in both directions. If the server's `.env` sets `EXCHANGERATE_API_KEY`, that key is used instead of
the saved one.

You can also click **Sync now** on **Finance > Exchange Rates**; the page shows the time of the last sync.

With only the functional currency in use, you don't need a key.

---

## Enter rates by hand

![The Exchange Rates page with a rate added by hand](images/multi-currency-fx-rates.png)

1. Open **Finance > Exchange Rates**.
2. In **Add or update a rate manually**, enter **From** and **To** (or click a pair under **Quick fill**).
3. Enter the **Rate** (1 unit of From = ? units of To), for example 1 USD = 150 JPY.
4. Choose the **Effective date** and click **Save rate**.

Saving a rate for a pair and date that already has one updates it. The page lists rates grouped by pair, with
the effective date, the rate and its source (`manual`, or `exchangerate-api` for synced rates). Use the delete
button on a row to remove a wrong rate.

When automatic sync is on, a sync replaces any rate for today's date, including one you entered by hand.
Enter manual rates for past dates, or turn off the sync (remove the key) if you set rates yourself.

You need the `finance:manage_fx` permission to add or delete rates. **Sync now** is for administrators.

---

## Which rate a document uses

When a document needs a rate, Granary looks for:

1. the rate you typed on the document, if any;
2. otherwise the most recent rate for that pair on or before the document's date;
3. otherwise the inverse pair's rate (for example JPY/USD when USD/JPY is missing), turned around.

If none of these exists, the action fails with a message like:

> No USD/JPY exchange rate is on record for 2027-03-31. Add it under Finance → Exchange Rates, or enter the
> rate on the document.

Add the rate, or type it on the document, and try again.

Forms that take a foreign currency show a rate field labeled with the direction, for example **Rate**
"1 USD = ? JPY". Next to it, **Fetch the spot rate** (on invoices) or **Fetch the rate** (on expenses and
journal entries) fills in the most recent rate on record for the date. Rate fields appear on:

- invoices (the **Rate** under **Currency**);
- payments on a foreign-currency invoice (**Exchange rate**, blank uses the rate on record);
- expense claims;
- manual journal entries.

---

## FX gain and loss

A foreign-currency invoice records its rate when it is created. When the customer pays, the payment has its
own rate (typed, or on record for the payment date). If the two differ, the receivable is worth a different
amount in your functional currency than when you invoiced it.

Example: Kumo Trading Co. (functional currency JPY) invoices a customer USD 1,000 at 150 JPY. The customer
pays a month later when the rate is 152. Granary books the JPY 2,000 difference as an FX gain. At 148 it would
be a JPY 2,000 loss.

- With automatic posting on, the payment's journal entry includes the difference on the account mapped to the
  `fx_gain_loss` role (account 4500 in the default chart). Vendor invoices work the same way, mirrored: a
  higher rate when you pay a vendor is a loss.
- Without automatic posting, recording the payment posts a separate FX entry named "FX gain on invoice ..." or
  "FX loss on invoice ...".

Map the `fx_gain_loss` role in **Finance > Chart of Accounts > Account mapping**; see [Finance](Finance#account-mapping).

---

## Rules worth knowing

- **Payments are in the invoice's currency.** If the customer paid in another currency, convert the amount
  received to the invoice's currency before you record it.
- **An invoice's currency can't change** after it is created. Delete the draft and create a new one.
- **A bank account's currency can't change** once payments or statements are recorded on it.
- **Reports are in the functional currency**: P&L, balance sheet, trial balance, AR aging and general ledger.
- **Budgets are in the functional currency only**: a budget line in another currency is refused.
- **Totals are never added across currencies.** Lists that hold several currencies (purchase orders, work
  orders, vendor invoices, fuel costs) show one total per currency.
- **A purchase order created from an RFQ** must use quotes in the PO's currency, and a vendor invoice must be in
  the currency of its order. See [Procurement](Procurement).

---

## Settings that affect this module

| Setting | Key | Where | What it does |
|---|---|---|---|
| Functional currency | `currency.functional` | Money & tax > Currencies | The ledger and report currency. Locked once entries are posted |
| Active currencies | `currency.active` | Money & tax > Currencies | The currencies offered in forms (default USD, EUR) |
| ExchangeRate API key | `currency.api_key` | Money & tax > Currencies | Key for automatic rates, stored encrypted |
| Decimal places per currency | `currency.decimals` | Money & tax > Accounting | Rounding per currency |

The environment variable `EXCHANGERATE_API_KEY` in `.env`, when set, takes priority over the saved key. See
[Configuration](Configuration).

---

## Permissions

| Permission | Allows |
|---|---|
| Administrator | Change the currency settings, test the API key, start a sync |
| `finance:manage_fx` | Add and delete exchange rates by hand |

Anyone signed in to the ERP can look up a rate (the **Fetch** buttons on forms).

---

## Related pages

- [Finance](Finance): invoices, payments, journal entries and account mapping
- [Procurement](Procurement): purchase orders, work orders and vendor invoices in foreign currencies
- [Configuration](Configuration): settings and environment variables

<!-- nav:bottom -->
---

← Previous: [Finance](Finance) | Next: [Procurement](Procurement) →
<!-- /nav:bottom -->
