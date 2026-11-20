# Projects

<!-- nav:top -->
[Home](Home) › Using Granary › Work together › **Projects**

**On this page:** [AI in this module](#ai-in-this-module) · [Find your way around](#find-your-way-around) · [Create a project](#create-a-project) · [Work on the board](#work-on-the-board) · [Milestones](#milestones) · [Files](#files) · [Log time](#log-time) · [My Tasks](#my-tasks) · [Delete a project](#delete-a-project) · [Settings that affect this module](#settings-that-affect-this-module) · [Permissions](#permissions)
<!-- /nav:top -->

The Projects module tracks internal and customer work: projects with a budget and a timeline, tasks on a
board, milestones, files, and the time people spend. Each logged hour adds to the project's cost to date, so
you can compare effort against budget. Project managers and team members use it every day; managers use the
time log reports to see where the hours go.

![All Projects list with saved views and status chips](images/projects-list.png)

---

## AI in this module

Nary can read your projects and start a project when a deal is won. See [AI Across Granary](AI-Across-Granary).

- **Ask Nary**: "What are my projects?" or, on a project page, "Summarize this project". Nary shows only the
  projects you may see (`projects:view_all` or `projects:view_own`).
- **Kickoff from a won deal**: when a deal is won, Nary writes a handoff package on the deal (scope sold and not
  sold, success criteria in the customer's words, commitments, red flags) and proposes the kickoff as a card: a
  project for the deal, its project manager from the SOP roles and the kickoff tasks in
  `nary.handoff.kickoff_tasks`. You can also ask for it in the chat (needs `projects:create`).
- **Ask AI about an attachment**: on a project's PDF or image attachment, click the sparkle button
  (**Ask AI about this document**).

---

## Find your way around

Open **Projects** in the sidebar. It has three pages:

| Page | What it shows |
|---|---|
| **All Projects** | Every project you can see, with status, customer, project manager (PM), due date and budget |
| **My Tasks** | The tasks assigned to you, across all projects |
| **Time Logs** | Time entries, totals and a breakdown by project |

On **All Projects**, the tiles at the top count **Active**, **Planning**, **Completed** and **Overdue** projects.
A project is overdue when it is active and its end date has passed. Use the saved view tabs (**All**,
**Active**, **Planning**, **Overdue**), the status chips (**On hold**, **Completed**, **Cancelled**,
**Archived**) and the **Search projects** box to narrow the list. Click a row to see its details, then click
**Open project**.

The **Due** column shows the end date with a small bar for how much of the planned timeline has passed. It
turns red when the project is overdue.

---

## Create a project

1. Open **Projects > All Projects** and click **New project**.
2. Enter a **Project name**, for example "Office fit-out, Shibuya branch".
3. Optional: set the **Start date** and **End date**, the **Currency** (it defaults to your functional
   currency, for example JPY) and the **Budget**.
4. Add a short **Description**.
5. Click **Create project**.

Granary gives the project a code (shown under its name). The budget is rounded to the currency's decimal
places. A negative or non-numeric budget is refused.

You need the `projects:create` permission.

---

## Work on the board

Open a project. The header shows the timeline bar, the **Tasks complete** progress, the project manager, the
customer and the budget. Below it are the tabs **Board**, **Tasks**, **Milestones** and **Files**.

![Project board with columns, cards and filters](images/projects-board.png)

On the **Board** tab:

- Drag a card from one column to another to change its status. On a phone, swipe between columns and use the
  card's status menu.
- Add a task directly in a column: type in **What needs doing?** and confirm.
- Filter the board with **Search**, the **Assignee** list (**Anyone**, **Unassigned** or a person), the
  **Priority** list and the **Epic** list. Click **Clear** to reset the filters.
- Click **Epics** to create, rename, recolor or delete epics (groups of related tasks).
- Click **Columns** to customize the board: add a column, rename, reorder or recolor it. A board starts with
  the default columns (**To do**, **In progress**, **In review**, **Done**, **Cancelled**). You cannot delete a
  column while tasks are in it: move them first.

### Add a task with details

1. Click **Add task** in the project header.
2. Enter a **Title**, and optionally a **Priority** (Low, Medium, High, Critical), a **Due date** and
   **Estimated hours**.
3. Click **Add task**. The new task starts in **To do**.

### Open a task

Click a card to open its detail drawer. You can change the **Title**, **Status**, **Priority**, **Assignee**,
**Epic**, **Estimate (h)**, **Due date**, **Labels** (comma separated) and **Description**, or delete the
task. The drawer also has three sections:

- **Comments**: write a comment and use `@` to mention a colleague. Each person you mention gets a
  notification. Only the author can edit or delete a comment.
- **Activity**: a feed of changes, such as status, priority and assignee changes, and new comments.
- **Files**: attachments for this task.

If you are the assignee but do not hold `projects:edit`, you can still move your own task between columns.
Every other change needs `projects:edit`.

### Tasks tab

The **Tasks** tab lists every task in a table. Use the status chips (**To do**, **In progress**,
**In review**, **Done**, **Cancelled**) to filter it, and change a task's status from the status menu in its
row.

---

## Milestones

1. Open the project and go to the **Milestones** tab.
2. Click **Add milestone**.
3. Enter a **Name** (for example "Phase 1 delivery") and a **Due date**. Both are required.

Each milestone shows **Open**, **Done** or **Overdue** (open and past its due date). Adding milestones needs
the `projects:manage_milestones` permission.

---

## Files

The **Files** tab holds documents for the whole project, such as a floor plan or a signed scope. Files are
kept in Granary's S3 storage.

---

## Log time

You can log time from two places:

- In a project, click **Log time**. Pick a **Task (optional)** or leave **Project level (no specific task)**,
  enter the **Hours** (for example 1.5), the **Date** and **Notes**, and save.
- In **Projects > Time Logs**, click **Log time**, pick the **Project**, and enter the **Hours**, **Date** and
  **Description**.

When you log time:

- The task's actual hours go up by the hours you entered.
- The project's cost to date goes up by the hours times your hourly cost. Your hourly cost is your employee
  base salary divided by the **Working hours in a month** setting, converted into the project currency. If
  you have no employee record, Granary uses **Hourly cost without an employee record** instead.
- If an exchange rate is missing for the conversion, the entry is refused. Add the rate in
  **Finance > Exchange Rates** and try again.

![Time Logs page with totals and the By project panel](images/projects-time-logs.png)

**Time Logs** shows **Total hours**, **This week**, **Total entries** and **Projects**, a table of entries
(**Date**, **Project**, **Work**, **Hours**, **By**) and a **By project** summary. Without the
`projects:view_time` permission you see only your own entries.

---

## My Tasks

**Projects > My Tasks** lists the tasks assigned to you across all projects. The tiles show **Total**,
**In progress**, **Overdue** and **Done**. Use the views (**Open**, **Overdue**, **Done**, **All**), the status
chips and **Group by project**. Click **Move to ...** on a row to advance a task one step (To do, In progress,
In review, Done).

---

## Delete a project

On **All Projects**, click the delete icon on the row and confirm with **Yes**. Active projects cannot be
deleted: put the project on hold, complete or cancel it first. Deleting a project also removes its tasks.
Deleting needs `projects:edit`.

---

## Settings that affect this module

Open **Settings > General** and search for the setting name.

| Setting | Key | What it does |
|---|---|---|
| Working hours in a month | `projects.monthly_hours` | Hourly cost is the employee's monthly salary divided by this (default 173). It cannot be 0. |
| Hourly cost without an employee record | `projects.default_hourly_cost` | The hourly cost, in the functional currency, for people with no employee record. Logging time fails until it is set. |

The functional currency and exchange rates (see [Finance](Finance)) also affect the cost to date.

---

## Permissions

| Permission | Allows |
|---|---|
| `projects:view_own` | Used by Nary's project tools. In the app, anyone without `projects:view_all` sees only the projects they are a member of |
| `projects:view_all` | See every project and its board, comments and activity |
| `projects:create` | Create projects |
| `projects:edit` | Change projects and tasks, add and delete tasks, delete projects |
| `projects:manage_milestones` | Add milestones |
| `projects:log_time` | Log time |
| `projects:view_time` | See everyone's time entries, not only your own |

Portal users (partners and vendors) never reach this module. Superadmins can do everything.

---

## Related pages

- [R&D](RnD): the same board, comments and activity feed for product development work
- [HR and Payroll](HR-and-Payroll): employee salaries drive the hourly cost
- [Finance](Finance): functional currency and exchange rates
- [Collaboration](Collaboration): channels and video calls for the project team

<!-- nav:bottom -->
---

← Previous: [Face Clock-In](Face-Clock-In) | Next: [R&D](RnD) →
<!-- /nav:bottom -->
