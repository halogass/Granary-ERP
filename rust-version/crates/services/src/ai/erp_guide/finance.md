# Finance: invoices, payments, schedules, expenses, ledger, tax, FX, assets, budgets

Finance bills customers, records what is paid, reimburses expenses and keeps the general ledger, with taxes and exchange rates from settings.

**Invoices** (/finance/invoices)
- Types: commercial, proforma, credit_note, debit_note. Statuses: draft, sent, partially_paid, paid, overdue, cancelled, refunded (also superseded and converted for proformas).
- **Issuing** (draft → sent, finance:send_invoice): the Owner approves every invoice and credit note first (WhatsApp ping above the policy amount); the approver is stamped as authorised by. A commercial invoice is then posted to the ledger.
- Manual moves: sent, partially_paid or overdue → paid (finance:mark_paid); draft → cancelled; an unpaid sent/overdue proforma → cancelled.
- Overdue is set daily for sent or partially paid invoices past due, with reminder emails on the `finance.invoice_reminder_days` schedule.
- A sent proforma can be revised (superseded + new draft). A commercial invoice is never revised: issue a credit note (full reversal, one per invoice; anything already paid becomes refund due). A proforma converts to a commercial invoice.
- Creating needs finance:create_invoice.

**Payments** (/finance/payments): recorded as confirmed (finance:confirm_payment) against a sent invoice (not a draft, cancelled, superseded, converted or a credit note); the invoice becomes paid or partially paid and the payment is posted. A payment can be reversed.

**Payment schedules** (/finance/schedules): milestones of a quotation (on order, delivery, milestone or date; pending, invoiced, paid, overdue, waived). Raising a proforma for a milestone marks it invoiced.

**Expenses** (/finance/expenses): submitted as pending (finance:submit_expense); approval chain line manager → Finance Admin → Owner when the claim or one of its lines is over the policy limits or the FX rate is missing; claims over the pre-approval limit must be pre-approved. Approved expenses post to the ledger; reimbursing needs finance:confirm_payment. A new claim goes to its approval chain the moment it is saved (tick "pre-approved" above the pre-approval limit, or it is refused); the Approve/Reject buttons act on that chain.

**Ledger** (/finance/journal, /finance/periods, /finance/accounts, /finance/reports)
- Journal entries: draft, posted, reversed. Post, unpost and reverse need finance:post_journal; lines balance per currency; the period must be open (a locked period blocks posting).
- Automatic posting (invoice issued, payment, credit note, refund, vendor bill, expense, payroll, depreciation, disposal) only from `finance.gl_autopost_from` (blank = off), using the account role mappings; an unmapped role is an error.

**Tax, FX, bank, assets, budget**
- Taxes are records (for example VAT10, a withholding tax on services, payroll income tax); default sales tax per currency in `finance.default_sales_tax`. Managed at /finance/taxes (finance:manage_tax_config).
- Exchange rates sync every 6 hours (the active currencies, for example USD, EUR, GBP, JPY) and can be entered by hand (finance:manage_fx); a quotation keeps the rate of its issue date.
- Bank reconciliation (/finance/bank, finance:manage_bank): reconcile only when every line is matched or excluded.
- Fixed assets (finance:manage_assets): depreciation and disposal post to the ledger.
- Budgets (finance:manage_budget): draft → approved → active → closed; lines editable only in draft.
