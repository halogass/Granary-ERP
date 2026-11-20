# R&D

<!-- nav:top -->
[Home](Home) › Using Granary › Work together › **R&D**

**On this page:** [AI in this module](#ai-in-this-module) · [Find your way around](#find-your-way-around) · [Product lines](#product-lines) · [Create an R&D project](#create-an-rd-project) · [Work in a project](#work-in-a-project) · [The backlog across projects](#the-backlog-across-projects) · [Connect GitLab](#connect-gitlab) · [Two-way issue sync](#two-way-issue-sync) · [Settings that affect this module](#settings-that-affect-this-module) · [Permissions](#permissions)
<!-- /nav:top -->

The R&D module tracks product development: a shared catalog of product lines, R&D projects with a backlog,
sprints, releases and a roadmap, and an optional link to a self-hosted GitLab server. With the link on, the
backlog and the GitLab issues of a repository stay in step both ways, and each project shows its build
(pipeline) status. Engineers and product owners use it; whoever runs the GitLab server sets up the link once.

![R&D projects list](images/rnd-projects-list.png)

---

## AI in this module

- **Tool ideas from Nary**: when you ask Nary for something none of its tools can do, it says so and files the
  idea in the R&D backlog of the project named in `nary.skills.tool_ideas_project`, labelled `nary-idea`. A
  repeated idea adds its example to the open item. Nary never writes or changes code. The setting is empty by
  default, so nothing is filed until you name a project.
- **Ask AI about an attachment**: on an R&D project's PDF or image attachment, click the sparkle button
  (**Ask AI about this document**).

See [AI Across Granary](AI-Across-Granary).

---

## Find your way around

Open **R&D** in the sidebar:

| Page | What it shows |
|---|---|
| **Projects** | Every R&D project with its product, status, latest version and items done |
| **Backlog** | All backlog items across all R&D projects |
| **Product Lines** | The shared catalog of product lines |

The GitLab link is set up under **Settings > GitLab Integration**.

---

## Product lines

Product lines are a shared catalog used across R&D, manufacturing and products. For Kumo Trading Co. these
could be "Office furniture", "Printers and copiers" and "Internal tools".

1. Open **R&D > Product Lines** and click **New product line**.
2. Enter a **Code**, a **Name**, a **Category**, a **Color** and a **Description**, and keep **Active** on.
3. Click **Create**.

Anyone signed in can read the catalog. Only the Owner or the Executive role can add, change or delete product
lines. Inactive lines are hidden from pickers.

---

## Create an R&D project

1. Open **R&D > Projects** and click **New project**. The **New R&D project** dialog opens.
2. Enter a **Project name** (for example "Order portal v2"), a **Code** and, if it belongs to one, a
   **Product** (a product line).
3. Add a **Description** of the scope and goals.
4. Click **Create project**. The code and the name are required.

The code is stored in capitals and must be unique: a code that is already in use is refused. The list shows
the tiles **Total projects**, **Active**, **Total items** and **Products**. Use the tabs (**All projects**,
**Active**, **Paused**, **Archived**) and **Search projects** to narrow it, then click **Open project**.

---

## Work in a project

The project page shows the status and code, the latest released version, and a button per linked GitLab
repository with its last pipeline status (**Passed**, **Failed**, **Running**, **Pending**). If a sprint is
running, a banner shows its name, goal, end date and a **Complete sprint** button.

![R&D project with the backlog board](images/rnd-project-board.png)

The tabs are **Backlog board**, **Sprints**, **Releases**, **Roadmap** and **Files**.

### Backlog board

The backlog board has the columns **Backlog**, **Refined**, **In Sprint**, **In Progress**, **Review** and
**Done**. It works like the [Projects](Projects) board: drag cards between columns, filter by assignee,
priority and epic, manage **Epics** and **Columns**, and click a card to open its drawer with **Comments**
(use `@` to mention someone), **Activity** and **Files**.

To add an item with details:

1. Click **Add item**.
2. Enter a **Title**, then pick a **Type** (Feature, Bug, Tech debt, Research, Spike, Docs, Security or
   Internal tool) and a **Priority** (Low, Medium, High, Critical).
3. Optional: enter **Points** (story points), pick a **Sprint** (or leave it unassigned in the backlog), and
   add **Labels**, comma separated.
4. Click **Add item**.

Completed or released items cannot be deleted.

### Sprints

1. On the **Sprints** tab, click **New sprint**.
2. Enter a **Sprint name**, a **Sprint goal**, the **Start date** and **End date**, and a
   **Velocity target (points)**.
3. Click **Create sprint**. The sprint starts in planning.
4. Click **Start** on the sprint when work begins. Only a sprint in planning can be started.
5. When the sprint ends, click **Complete** (or **Complete sprint** on the banner). Enter the
   **Velocity achieved (points)** and the **Retrospective notes**, then click **Complete sprint**.

When you complete a sprint, every item in it that is not Done goes back to the backlog as **Refined**, ready
to plan again. Only an active sprint can be completed.

### Releases

1. On the **Releases** tab, click **New release**.
2. Enter the **Version** (required, for example 2.1.0), a **Release date**, a **Release name** and
   **Release notes** (Markdown).
3. Click **Create release**.
4. Click **Publish** when the version ships. The release becomes released, gets today's date if it had none,
   and becomes the project's latest version.

A release that carries a GitLab tag shows a button that opens the tag.

### Roadmap and files

The **Roadmap** tab shows the project's roadmap notes. The **Files** tab holds attachments for the project.

---

## The backlog across projects

**R&D > Backlog** lists all items from all projects with **Item**, **Type**, **Priority**, **Points** and
**Status**. Filter by project and priority, or search by title. Items linked to GitLab have a link that opens
the GitLab issue.

---

## Connect GitLab

You need `rnd:manage` for every step. Open **Settings > GitLab Integration**. The **How this works** box on the
page repeats these steps.

### 1. Add the server

1. In GitLab, create a personal access token with the `api` scope (your avatar > **Edit profile** >
   **Access tokens**).
2. In Granary, click **Add server**. Enter a **Display name**, the **GitLab URL** (for example
   `https://gitlab.example.com`) and the **Personal access token**, then click **Add server**.
3. Open the server (**Edit**), click **Generate** next to **Webhook secret**, save, and copy the secret.

Until a webhook secret is set, Granary refuses every GitLab webhook and the page warns you.

### 2. Link repositories

1. Click **Add repository**.
2. Pick the project from the server's list (or click **Enter the project by hand instead** and fill in the
   **GitLab project ID**, **Full path**, **Web URL** and **Default branch**).
3. Choose its **R&D project** and save.

Click **Sync** on a repository to pull its recent pipelines. After that, GitLab reports new pipelines by
itself.

### 3. Add the webhook in GitLab

In each GitLab project, open **Settings > Webhooks > Add new webhook**:

- **URL**: the webhook URL shown in the server's details (use **Copy webhook URL**).
- **Secret token**: the webhook secret from step 1 (use **Copy secret**). One secret serves every project on
  that server.
- **Trigger**: tick Push events, Tag push events, Comments, Merge request events, Issues events and Pipeline
  events.
- Keep **Enable SSL verification** on, then click **Add webhook**.

Check it with **Test > Pipeline events**: GitLab should show HTTP 200. A 401 means the secrets differ, or no
secret is set in Granary.

![GitLab Integration settings with a server and linked repositories](images/rnd-gitlab-settings.png)

---

## Two-way issue sync

Turn on **Sync backlog with GitLab issues** on a linked repository. Granary asks **Turn on the issue sync?**;
click **Turn on and import**.

- **First sync**: every GitLab issue becomes a backlog item, and every backlog item gets an issue (Done items
  as closed issues). You get a notice with the numbers.
- **What follows**: title, description with acceptance criteria, status, priority, type, labels, assignee and
  comments. Story points, sprint, epic and board order stay in Granary only.
- **Status**: Done closes the issue. Other statuses show as GitLab labels such as `status::in_progress`, which
  a GitLab issue board can use as columns.
- **Both sides changed**: the newer edit wins. The other value is kept in the item's history.
- **Deleting**: an item deleted in Granary closes its issue with the label `erp::deleted`. An issue deleted
  in GitLab leaves the item in Granary, unlinked.
- **People**: users are matched by email where GitLab shows it. Set the rest by hand in the **People**
  section. Comments written in Granary are posted by the access token's account, as "Name (ERP): ...".
- **Timing**: with the Issues events and Comments triggers on, changes arrive at once. Granary also pulls
  recently updated issues on a schedule, so a missed webhook is caught.

If GitLab refuses a change, Granary retries it later and shows **Last error** on the repository, and a count of failing items. Click **Sync issues now** to run the sync at once. The sync is refused when it is off for the repository, the
repository has no R&D project, or its server has no token.

---

## Settings that affect this module

Under **Settings > General**, area **Integrations & access**, block **GitLab**:

| Setting | Key | What it does |
|---|---|---|
| Issue sync: items per minute | `rnd.gitlab.sync_batch` | Backlog items and comments sent to each repository per minute (default 50); the rest go the next minute |
| Issue sync: pull every | `rnd.gitlab.pull_minutes` | How often recently updated issues are read from GitLab (default 15 minutes) |
| Issue sync: longest retry wait | `rnd.gitlab.retry_max_minutes` | A refused item is retried after 1, 2, 4 ... minutes, up to this (default 60) |

The GitLab servers, tokens, webhook secrets and repositories are set on **Settings > GitLab Integration**. The
access token and webhook secret are stored encrypted.

---

## Permissions

| Permission | Allows |
|---|---|
| `rnd:view` | See R&D projects, sprints, backlog, releases, linked repositories, pipelines and GitLab events; comment on items |
| `rnd:manage` | Everything in `rnd:view`, plus create and change projects, sprints, items, releases, board columns and epics, and set up GitLab |

Product lines are edited only by the Owner or the Executive role. The GitLab webhook itself needs no user: it
is checked against the webhook secret.

---

## Related pages

- [Projects](Projects): the same board, comments and activity feed for delivery projects
- [Knowledge and Documents](Knowledge-and-Documents): specifications and SOPs
- [Collaboration](Collaboration): channels and calls for the team

<!-- nav:bottom -->
---

← Previous: [Projects](Projects) | Next: [Knowledge and Documents](Knowledge-and-Documents) →
<!-- /nav:bottom -->
