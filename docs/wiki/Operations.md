# Operations

<!-- nav:top -->
[Home](Home) › Using Granary › Buy and make › **Operations**

**On this page:** [AI in this module](#ai-in-this-module) · [Fleet](#fleet) · [Maintenance](#maintenance) · [Helpdesk](#helpdesk) · [Settings that affect this module](#settings-that-affect-this-module) · [Permissions](#permissions)
<!-- /nav:top -->

The Operations area groups three day-to-day modules. **Fleet** keeps your company vehicles: who drives them,
fuel, service history and insurance and registration dates. **Maintenance** keeps your equipment running:
an equipment register, maintenance requests from anyone, work orders and recurring preventive schedules.
**Helpdesk** handles support tickets from customers, who open them on a public support form and follow them
on a tracking page. Office managers, facility and maintenance staff, and support agents use these modules.

![The helpdesk ticket list with a ticket's conversation open](images/operations-helpdesk-tickets.png)

---

## AI in this module

Nary has no fleet, maintenance or helpdesk tools yet: it can't read or change vehicles, maintenance jobs or tickets. In the chat it can explain how this module works, from the ERP guide built into Granary. See [AI Across Granary](AI-Across-Granary) for what the AI does in the other modules.

---

## Fleet

Open **Fleet** in the sidebar. It has **Vehicles**, **Assignments**, **Fuel Logs** and **Service Records**.

![The fleet vehicle list with expiring documents flagged](images/operations-fleet-vehicles.png)

### Add a vehicle

1. Open **Fleet > Vehicles** and click **Add vehicle**.
2. Enter the **Plate number**, **Make** and **Model** (all three are required), the **Year**, the **Color** and
   the **Odometer (km)**.
3. Pick the **Vehicle type** (car, motorcycle, truck, van, other) and the **Fuel type** (petrol, diesel,
   electric, hybrid).
4. Enter the **Insurance expiry** and the registration expiry date.
5. Click **Add vehicle**.

The tiles show the **Total fleet**, **Active**, **In maintenance**, **Expiring soon** and **Fuel this month**
(one figure per currency). A vehicle whose insurance, registration or next service is due within the fleet
warning period is marked **Expiring soon**. The tabs are **All**, **Active**, **Maintenance** and
**Inactive**.

On a vehicle's details, click **Edit** to change it, **Maintenance** to take it out of service, or **Mark
active** to put it back.

### Assign a vehicle

1. Open **Fleet > Assignments** and click **Assign vehicle**.
2. Pick the **Vehicle** and, optionally, the **Employee** (or leave **No specific employee**).
3. Enter the **Start date**, an optional **End date** (leave it blank for an ongoing assignment) and the
   **Purpose** (for example "Sales visits").
4. Click **Assign**.

Click **End** on an assignment to close it. The vehicle list shows each vehicle's current driver and purpose.

### Log fuel

1. Open **Fleet > Fuel Logs** and click **Add fuel log**.
2. Pick the **Vehicle** and enter the **Date**, the **Odometer (km)** (optional), the **Liters**, the **Cost**
   and the **Fuel station**.
3. Click **Add log**.

The vehicle, liters and cost are required. A reading higher than the vehicle's odometer updates it. Use
**Filter by vehicle** to see one vehicle's logs and totals.

### Record a service

1. Open **Fleet > Service Records** and click **Add service record**.
2. Pick the **Vehicle** and enter the **Service date**.
3. Pick the **Service type** (Oil Change, Tire, Brake, Inspection, Repair, Other) and add a **Description**.
4. Enter the **Cost**, the **Workshop**, the **Odometer (km)** and the **Next service date**.
5. Click **Add record**.

Recording a service raises the odometer when the reading is higher and ends the vehicle's maintenance status.
The **Due soon** chip shows vehicles whose next service is close.

---

## Maintenance

Open **Maintenance** in the sidebar. It has **Equipment**, **Requests**, **Work Orders** and **Schedules**.

### Register equipment

1. Open **Maintenance > Equipment** and click **Add equipment**.
2. Enter the **Name** (for example "Forklift #1") and the **Code** (for example `EQ-001`). Both are required.
3. Optional: the **Type**, **Brand**, **Model**, **Serial number**, **Location** and **Notes**.
4. Click **Add**.

The tabs are **All**, **Active**, **In maintenance** and **Decommissioned**.

### Request maintenance

Anyone with the request permission can report a problem.

1. Open **Maintenance > Requests** and click **Submit request**.
2. Pick the **Equipment** and the **Priority** (Low, Normal, High, Critical).
3. Describe the issue in **Description** and click **Submit**.

### Review a request

![Reviewing a maintenance request and creating a work order](images/operations-maintenance-review.png)

1. On a submitted request, click **Review**.
2. Choose the **Decision**.
3. To approve, pick the **Work type** (Corrective (repair) or Preventive) and the **Schedule date**, then click
   **Approve and create WO**. Granary creates a work order and marks the equipment as in maintenance.
4. To reject, give a **Rejection reason**.

The tabs are **Submitted**, **Under review**, **Approved**, **Rejected** and **All**. A request that was
already decided can't be reviewed again.

### Work orders

You can also create a work order directly: open **Maintenance > Work Orders**, click **New work order**, pick
the **Equipment**, the **Type** (Corrective or Preventive), the **Scheduled date** and a **Description**, then
click **Create**.

| Status | Actions |
|---|---|
| Planned | **Start**, **Cancel** |
| In progress | **Complete**, **Cancel** |

When you click **Complete**, enter the **Downtime (hours)**, the **Total cost** and **Completion notes**, then
click **Mark completed**. Completing a work order puts the equipment back to active.

### Preventive schedules

1. Open **Maintenance > Schedules** and click **New plan**.
2. Pick the **Equipment** and enter the **Plan name** (for example "Quarterly inspection").
3. Set **Every** and the **Frequency unit** (Days, Months or Hours (usage)), for example every 3 months.
4. Enter the **First due date** and a **Description**, then click **Create plan**.

When the work is done, click **Done** on the plan. Granary records a completed preventive work order and moves
the next due date forward by the frequency. The tiles and chips show **Overdue** plans and plans **Due soon**.

---

## Helpdesk

Open **Helpdesk** in the sidebar. It has **Tickets** and **Teams & SLA**.

### Set up teams and SLA policies

1. Open **Helpdesk > Teams & SLA**, click **New team** and enter the **Team name** (for example "Customer
   Support"), the **Inbox email** (for example `support@example.com`) and a **Description**. Click **Create**.
2. Under **SLA policies**, click **Add SLA**, enter a **Name**, the **Priority**, the **First response (h)** and
   the **Resolution (h)** targets in hours, then click **Create policy**.

A team can have one SLA policy per priority. To change a policy, delete it and add it again. A team can be
deactivated.

### Work on tickets

Tickets come in from the public support form (below). The tabs are **New**, **Open**, **Pending**,
**Resolved**, **Closed** and **All**.

1. Click a ticket to open it. You see the customer, the team and category, the SLA deadlines and the
   conversation.
2. Choose **Reply to customer** or **Internal note**, write the message and click **Send** (or press
   Ctrl+Enter). Internal notes are never shown to the customer.
3. Use **Open**, **Resolve** and **Close** to move the ticket along.

The first reply to the customer moves a new ticket to open and records the first response time. Each ticket
shows its **First response** and **Resolution** deadlines from the team's SLA policy for its priority; a
missed deadline shows **SLA breached**.

Replies are not emailed: the customer reads them on the ticket's tracking page.

### The public support form

Anyone can open a ticket at `/support`, without an account.

![The public support form](images/operations-support-form.png)

1. The customer fills in **Your information**: **Full name**, **Email address** and, optionally, **Company or
   organization**.
2. Under **Your request**, they enter the **Subject**, pick the **Priority** (Low, Normal, High), optionally a
   **Team** and **Category**, and write the **Description**.
3. They click **Submit support request**.

The confirmation shows the **Ticket number** and a **Track my ticket** link (`/support/track/<token>`). The
tracking page shows the ticket's status, details and the conversation (without internal notes). Keep the link:
it is the only way to see the ticket without an account. Submissions are rate-limited per IP address.

Link to `/support` from your website or email signature so customers can find it.

---

## Settings that affect this module

Open **Settings > General**, then **Operations > Fleet & maintenance**.

| Setting | Key | What it does |
|---|---|---|
| Fleet expiry warning | `fleet.expiry_warning_days` | Days before an insurance, registration or service date that a vehicle shows as expiring soon (default 30) |
| Maintenance due soon | `maintenance.due_soon_days` | Days before a maintenance plan is due that it shows as due soon (default 7) |

Helpdesk teams and SLA targets are set on **Helpdesk > Teams & SLA**, not in Settings.

---

## Permissions

| Permission | Allows |
|---|---|
| `fleet:view` | See vehicles, assignments, fuel logs and service records |
| `fleet:manage` | Add, change and delete vehicles, assignments, fuel logs and service records |
| `maintenance:view` | See equipment, requests, work orders and schedules |
| `maintenance:request` | Submit maintenance requests |
| `maintenance:manage` | Manage equipment, review requests, run work orders and schedules |
| `helpdesk:view` | See tickets, teams and categories |
| `helpdesk:manage` | Reply to tickets, add internal notes, change and delete tickets |
| `helpdesk:configure` | Create and change teams, categories and SLA policies |

The public support form and tracking page need no account. Portal users never reach these modules.
Superadmins can do everything.

---

## Related pages

- [Manufacturing and Inventory](Manufacturing-and-Inventory): production equipment and stock
- [HR and Payroll](HR-and-Payroll): the employees vehicles are assigned to
- [CRM](CRM): customers who open support tickets
- [Configuration](Configuration): settings

<!-- nav:bottom -->
---

← Previous: [Manufacturing and Inventory](Manufacturing-and-Inventory) | Next: [HR and Payroll](HR-and-Payroll) →
<!-- /nav:bottom -->
