# Projects and R&D: delivery after a deal is won, and product development

Projects deliver what was sold: a Won deal is handed over to a project with phases, milestones, tasks and time logs. R&D tracks the company's own product development.

- **Projects** (/projects): planning, active, on_hold, completed, cancelled. Creating needs projects:create (starts in planning); editing needs projects:edit; an active project can't be deleted. Status can be set freely (no fixed order). Seeing all projects needs projects:view_all, otherwise only your own.
- **Phases** (not_started, in_progress, completed, skipped) and **milestones** (pending, achieved, missed, waived; projects:manage_milestones).
- **Tasks** (/projects/tasks): todo, in_progress, review, done, cancelled, with a priority.
- **Time logs** (/projects/time-logs): logging needs projects:log_time, seeing everyone's needs projects:view_time; cost uses `projects.monthly_hours` and `projects.default_hourly_cost`.
- **Handoff from a Won deal**: Nary writes a handoff package on the deal and proposes a kickoff card. Applying it creates one installation project in planning, with the PM and Tech Lead from Nary's role table, the deal value as budget and the kickoff tasks from `nary.handoff.kickoff_tasks`.
- **R&D** (/rnd/projects, /rnd/backlog, /rnd/product-lines): R&D projects (active, paused, archived), sprints, a backlog (backlog → refined → in_sprint → in_progress → review → done, or rejected/deferred), releases and GitLab pipelines. Permissions rnd:view and rnd:manage.
