# HR and Payroll

<!-- nav:top -->
[Home](Home) › Using Granary › People › **HR and Payroll**

**On this page:** [AI in this module](#ai-in-this-module) · [Find your way around](#find-your-way-around) · [Employees](#employees) · [Attendance](#attendance) · [Leave](#leave) · [Payroll](#payroll) · [Recruiting](#recruiting) · [Settings that affect this module](#settings-that-affect-this-module) · [Permissions](#permissions)
<!-- /nav:top -->

The HR module holds your people records and the work around them: employees, the org chart, attendance,
leave, monthly payroll with contributions and income tax, overtime, recruiting from hiring request to offer
letter, and recurring allowances. Every payroll rate (contributions, wage caps, income tax brackets, overtime
multiples) is a setting, so the same module fits any country. HR staff use it daily; managers use it to
request hires and approve leave; every employee uses it to request leave and clock in.

![Employees list](images/hr-employees-list.png)

---

## AI in this module

Nary has no HR or payroll tools yet: it can't read or change employees, attendance, leave or payroll. In the chat it can explain how this module works, from the ERP guide built into Granary. See [AI Across Granary](AI-Across-Granary) for what the AI does in the other modules.

---

## Find your way around

Open **HR** in the sidebar:

| Page | What it is for |
|---|---|
| **Org Chart** | The company structure by department, with coordinative links between departments |
| **Employees** | Employee records, face enrollment, allowances |
| **Attendance** | Attendance records, clock events and the review of unconfirmed clock-ins |
| **Payroll** | Monthly payroll runs |
| **Leave** | Leave requests and their review |
| **Hiring** | Hiring requests from managers |
| **Job Openings** | Open positions |
| **Recruitment** | The applicant pipeline |
| **Offer Letters** | Offers to candidates, approved by the Owner before they are issued |

---

## Employees

### Add an employee

1. Open **HR > Employees** and click **Add employee**.
2. Enter the **First name**, **Last name** and **Email** (for example `kenji.mori@example.com`), and
   optionally the **Phone**.
3. Pick the **Department**, the **Job title**, the **Employment type** (Full-time, Part-time, Contract,
   Intern), the **Start date** and who the person **Reports to**.
4. To let the person sign in, tick **Also create a system user account** and enter a **Temporary password**.
   They are asked to change it on first sign-in. The role bundle for the job title is assigned automatically.
5. Click **Add employee**.

A user with the same email cannot be created twice. The employee record links the sign-in to attendance,
leave, payroll and project time logs.

### Change or remove an employee

- Click **Edit** on a row to change the name, department, job title, phone or manager, then
  **Save changes**.
- Click **Allowances** to manage recurring allowances (see [Allowances](#allowances)).
- Click **Face ID** to enroll the face for clock-in (see [Face Clock-In](Face-Clock-In)).
- Click the delete icon and confirm. The record is removed from the lists and the linked sign-in is
  deactivated.

Use the tabs (**All**, **Active**, **Probation**, **On leave**) and **Search employees** to find people. Anyone
can open an employee record, but the salary is shown only to holders of `hr:view_compensation`.

Payroll uses each employee's base salary and salary currency from the employee record. The **Add employee**
form has no salary field: the values are set when the record is created through the API.

### Org chart

**HR > Org Chart** shows active, probation and on-leave employees grouped by department, with reporting lines.
Dashed lines are coordinative links, for example "Finance coordinates with Operations". A superadmin can add
and remove these with **Manage links**: pick the **From department** and the **To department** (they must
differ), add **Notes (optional)**, and click **Add link**.

![Org chart with departments and a coordinative link](images/hr-org-chart.png)

---

## Attendance

Employees clock in and out with their face, on their own device or on a kiosk tablet. That is described on
[Face Clock-In](Face-Clock-In). This section covers the HR side.

Open **HR > Attendance**. The **Records** tab shows each employee's day: **Date**, **Check in**,
**Check out**, **Hours**, **Late (min)**, **OT (hrs)** and **Status** (Present, Absent, Half day, On leave,
Holiday, Remote). Filter by **Month**, **Year** and employee.

- A clock-in after the **Work start time** setting is flagged late.
- A clock-out after midnight closes the previous day's shift if it began within the **Overnight shift window**.
- To record a day by hand, click **Log attendance**, pick the **Employee**, **Date** and **Status**, and
  enter the **Check in**, **Check out**, **Late (min)**, **Overtime (hrs)** and **Notes**.

The overtime hours on attendance records feed payroll. Days with status Holiday count as rest days for
overtime pay.

The other tabs, **Pending overrides**, **Clock events** and **Attempts**, are explained on
[Face Clock-In](Face-Clock-In#review-weak-matches-hr). **Kiosk clock** opens the kiosk screen in a new tab.

---

## Leave

### Request leave

1. Open **HR > Leave** and click **Request leave**.
2. Pick the **Leave type** (Annual, Sick, Unpaid, Maternity, Paternity, Emergency, Other).
3. Enter the **Start date**, **End date** and a **Reason**.
4. Click **Submit request**.

The number of days counts every calendar day from start to end. The end date cannot be before the start
date. Your account must be linked to an employee record.

### Review leave

1. Open **HR > Leave**. Use the **Pending** tab to see what waits for you.
2. Click **Review** on a request.
3. Pick the **Decision** (Approve or Reject), add review notes if you like, and click **Submit review**.

Only pending requests can be reviewed. You cannot approve your own leave request. Pending and rejected
requests can be deleted; approved ones cannot.

Without `hr:view_leave` or `hr:approve_leave` you see only your own requests.

---

## Payroll

### Before the first run

Payroll refuses to run while a setting it needs is empty or malformed, and the error names the setting. Set
these first, under **Settings > General**, area **People & payroll**:

- **Contributions**: the social security, health and pension contributions your jurisdiction requires.
  Each has an employee rate and an employer rate (percent of the fixed wage), an optional monthly wage cap,
  and flags for whether the employer share is taxable income and the employee share is tax-deductible. An
  empty list means no contributions.
- **Income tax**: `{"method": "none"}`, or progressive annual brackets with a yearly tax-free allowance per
  tax status, an optional work-expense deduction and rounding.
- **Overtime**: the hourly wage divisor, the overtime multiples per day type, and the rest days.

Check your local rates before the first run. Granary ships no country's rates.

### How pay is calculated

For each active employee (and anyone whose last day falls in the month):

- **Base pay**: the base salary, prorated by the days employed in the month, converted into the company's
  functional currency (for Kumo Trading Co., JPY). A missing exchange rate stops the run.
- **Allowances**: the employee's active recurring allowances.
- **Overtime**: the hourly wage (the monthly fixed wage divided by **Hourly wage divisor**) times the
  overtime hours from attendance, at the multiple for that day type.
- **Gross** = base pay + allowances + overtime.
- **Contributions** are charged on the fixed wage (base pay plus fixed allowances), up to each contribution's
  wage cap.
- **Income tax** is withheld on the annualized year-to-date income, so each month corrects the months before
  it. The final month of the year, or the month an employee leaves, settles the year on actual amounts.
- **Net salary** = gross minus the employee contributions and income tax.

### Run payroll

1. Open **HR > Payroll** and click **New payroll run**.
2. Pick the **Month** and **Year**, add **Notes** if you like, and click **Create and calculate**. Granary
   creates a draft run and calculates every line.
3. Open the run to check each employee's **Gross**, **Contributions**, **Income tax** and **Net salary**.
4. Click **Approve**. If an approval policy exists for payroll runs, the run goes through it; otherwise you
   need `hr:manage_compensation`.
5. When the salaries are paid, click **Mark paid**, enter the **Paid date** and pick the bank account in
   **Paid from**, then confirm. Granary posts the payroll journal entry to the ledger.

Only one run per month can exist (cancelled runs aside). Only a draft run can be approved, and only an
approved run can be marked paid.

![Payroll run with lines per employee](images/hr-payroll-run.png)

### Allowances

1. Open **HR > Employees** and click **Allowances** on the employee.
2. Enter a **Name** (for example "Transport"), the **Monthly amount** and **Currency**.
3. Tick **Fixed** if it is part of the fixed wage, and **Taxable** if it counts as taxable income. Keep
   **Active** on.

Fixed allowances count toward the contribution wage and the overtime hourly wage. Managing allowances needs
`hr:manage_compensation`.

---

## Recruiting

Recruiting follows four steps: a manager asks for headcount, HR posts an opening, applicants move through
the pipeline, and the chosen candidate gets an offer letter.

### 1. Request a hire (managers)

1. Open **HR > Hiring** and click **New request**.
2. Enter the **Position title**, **Department**, **Headcount**, **Type** and a **Justification** (why the
   hire is needed, and the impact if it is not filled).
3. Click **Submit request**.

The request goes to the Owner for approval (status **Awaiting Owner**), then to HR. Only holders of
`hr:request_hire` (management and team leads) can submit one.

### 2. Post a job opening (HR)

1. Open **HR > Job Openings** and click **Post opening**.
2. Enter the **Job title**, **Department**, **Employment type**, **Location** (for example "Tokyo / Remote"),
   **Headcount**, **Description** and **Requirements**.
3. Click **Post opening**.

Openings can be Open, Paused, Filled or Cancelled. Only open openings are visible outside the recruiting team.

### 3. Track applicants

1. Open **HR > Recruitment** and click **Add applicant**.
2. Enter the **Full name**, **Email**, **Phone**, pick the **Opening** and the **Source** (for example
   LinkedIn, Referral, Website), and add a **Resume URL**.
3. Click **Add applicant**.

To move an applicant along, click **Update stage**, pick the stage in **Move to stage**, give a
**Rating (1 to 5)** and **Notes**, and save. The stages are Applied, Screening, Phone screen, Interview 1,
Interview 2, Technical, Offer sent, Accepted, Hired and Rejected.

![Recruitment pipeline](images/hr-recruitment.png)

### 4. Make an offer

1. Open **HR > Offer Letters** and click **New offer**.
2. Pick the **Candidate**, then fill in the **Position**, **Department**, **Employment type**,
   **Gross per month** and **Currency**, the **Start date**, **Probation (months, max 3)**, **Benefits** and
   **Other terms**.
3. **Respond by** defaults to 5 business days from today.
4. Click **Create draft**.
5. Open the offer and click **Issue to candidate**. The Owner approves every offer before it is issued: if
   the offer is not yet approved, it is submitted for approval and you see a notice; issue it again once it
   is approved. Any change to the draft needs a new approval.
6. When the candidate answers, click **Record response** and pick **Accepted**, **Declined** or
   **Countered**, with notes.

The applicant's stage follows the offer: Offer sent when issued, Accepted when accepted. **Cancel offer**
needs a reason and stops its approvals counting. Only drafts and cancelled offers can be deleted.

---

## Settings that affect this module

Under **Settings > General**, area **People & payroll**:

| Setting | Key | What it does |
|---|---|---|
| Work start time | `hr.work_start_time` | Clock-ins after this time are flagged late (default 09:00) |
| Overnight shift window | `hr.overnight_shift_hours` | A clock-out after midnight closes yesterday's shift if it began within this many hours |
| Contributions | `payroll.contributions` | The list of contributions, with employee and employer rates, wage caps and tax flags |
| Income tax | `payroll.income_tax` | No income tax, or progressive brackets with allowances per tax status |
| Hourly wage divisor | `payroll.overtime_hours_divisor` | Hourly wage is the monthly fixed wage divided by this (for example 173) |
| Overtime multiples | `payroll.overtime_multipliers` | Per day type: cumulative hour bound and the multiple of the hourly wage |
| Rest days | `payroll.rest_weekdays` | Weekdays, Monday 0 to Sunday 6; days marked Holiday in attendance also count |

The face clock-in settings, office locations and kiosk tablets are in the same area: see
[Face Clock-In](Face-Clock-In#settings-that-affect-this-module).

Under **Settings > General**, area **Approvals & signing**, **Approval policies** decide who approves payroll
runs, hiring requests and offer letters.

The company time zone and functional currency (**Company & branding > Regional**) and the exchange rates in
**Finance > Exchange Rates** also affect payroll.

---

## Permissions

| Permission | Allows |
|---|---|
| `hr:view_all_employees` | See all employees and the org chart |
| `hr:manage_employees` | Add, edit and delete employees; enroll faces |
| `hr:view_compensation` | See salaries and payroll runs |
| `hr:manage_compensation` | Create, approve and mark paid payroll runs; manage allowances |
| `hr:view_attendance` | See everyone's attendance and clock events |
| `hr:manage_attendance` | Log attendance, manage office locations and kiosks, approve or reject clock events |
| `hr:submit_leave` | Request leave |
| `hr:view_leave` | See all leave requests |
| `hr:approve_leave` | Review leave requests |
| `hr:request_hire` | Submit hiring requests |
| `hr:manage_job_openings` | Post and manage job openings |
| `hr:manage_applicants` | Add applicants, move them through stages, manage offer letters |
| `hr:initiate_termination` | Request an employee's termination (the direct senior only, never HR) |

Managing coordinative links on the org chart is for superadmins only.

---

## Related pages

- [Face Clock-In](Face-Clock-In): clock-in on phones and kiosk tablets
- [Projects](Projects): employee salaries set the hourly cost of logged time
- [Finance](Finance): bank accounts, journal entries and exchange rates
- [Approvals and Signing](Approvals-and-Signing): approval policies for payroll runs, hiring requests and offers
- [Knowledge and Documents](Knowledge-and-Documents): HR policies and SOPs

<!-- nav:bottom -->
---

← Previous: [Operations](Operations) | Next: [Face Clock-In](Face-Clock-In) →
<!-- /nav:bottom -->
