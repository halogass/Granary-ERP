//! The task registry: Celery's `@celery.task(name=...)` table.
//!
//! A task is a [`TaskDef`]: its Celery name (`app.tasks.<name>`), its Python parameters (names and defaults,
//! so a job's `args`/`kwargs` bind exactly as Python binds a call), `max_retries`, and the function that runs
//! it. The function gets a [`TaskCtx`] (bound arguments, the retry count, the shared state, fresh DB sessions)
//! and returns an [`Outcome`]: done (the return value, logged like Celery's "Task ... succeeded") or a retry
//! with a countdown (`raise self.retry(countdown=...)`). An `Err` is the task raising: logged, and the job is
//! acknowledged (Celery `task_acks_late` acks a failed task too; there is no automatic retry).

use std::collections::HashMap;
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::{Map, Value};

use granary_core::db::DbSession;
use granary_core::state::AppState;

/// What a task returns (`Err` = the task raised).
pub type TaskResult = Result<Outcome, anyhow::Error>;

/// The task body: `|ctx| Box::pin(my_task(ctx))`.
pub type TaskFn = fn(TaskCtx) -> BoxFuture<'static, TaskResult>;

/// How a task run ended (when it did not raise).
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The return value (Python returns a short status string from most tasks).
    Done(Value),
    /// `raise self.retry(countdown=...)`: run the same call again after `countdown`, with `retries + 1`.
    Retry { countdown: Duration },
}

impl Outcome {
    pub fn done(v: impl Into<Value>) -> TaskResult {
        Ok(Outcome::Done(v.into()))
    }
}

/// One Python parameter: name and default (`None` = required). The default is JSON text (`"null"`,
/// `"false"`, `"\"attribute\""`).
#[derive(Debug, Clone, Copy)]
pub struct Param {
    pub name: &'static str,
    pub default: Option<&'static str>,
}

impl Param {
    /// A required parameter (`chat_id: str`).
    pub const fn req(name: &'static str) -> Param {
        Param {
            name,
            default: None,
        }
    }
    /// A parameter with a default, as JSON text (`late: bool = False` -> `Param::opt("late", "false")`).
    pub const fn opt(name: &'static str, default_json: &'static str) -> Param {
        Param {
            name,
            default: Some(default_json),
        }
    }
}

/// One registered task.
#[derive(Clone, Copy)]
pub struct TaskDef {
    /// The Celery name, `app.tasks.<name>`.
    pub name: &'static str,
    /// The Python signature (without `self` for `bind=True` tasks).
    pub params: &'static [Param],
    /// `max_retries` (Celery's default is 3; `None` = unlimited, as `max_retries=None`).
    pub max_retries: Option<u32>,
    pub run: TaskFn,
}

impl TaskDef {
    /// A task with Celery's default `max_retries=3`.
    pub const fn new(name: &'static str, params: &'static [Param], run: TaskFn) -> TaskDef {
        TaskDef {
            name,
            params,
            max_retries: Some(3),
            run,
        }
    }

    /// `max_retries=...` (`None` for unlimited).
    pub const fn max_retries(mut self, n: Option<u32>) -> TaskDef {
        self.max_retries = n;
        self
    }

    /// Python's call binding of `args`/`kwargs` to the parameters (`f(*args, **kwargs)`): a TypeError text
    /// when the call does not fit.
    pub fn bind(
        &self,
        args: &[Value],
        kwargs: &Map<String, Value>,
    ) -> Result<Map<String, Value>, String> {
        let short = self.name.rsplit('.').next().unwrap_or(self.name);
        if args.len() > self.params.len() {
            return Err(format!(
                "{short}() takes {} positional argument{} but {} {} given",
                self.params.len(),
                if self.params.len() == 1 { "" } else { "s" },
                args.len(),
                if args.len() == 1 { "was" } else { "were" }
            ));
        }
        let mut out = Map::new();
        for (p, v) in self.params.iter().zip(args) {
            out.insert(p.name.to_string(), v.clone());
        }
        for (k, v) in kwargs {
            if !self.params.iter().any(|p| p.name == k) {
                return Err(format!(
                    "{short}() got an unexpected keyword argument '{k}'"
                ));
            }
            if out.contains_key(k) {
                return Err(format!("{short}() got multiple values for argument '{k}'"));
            }
            out.insert(k.clone(), v.clone());
        }
        let mut missing = Vec::new();
        let mut bound = Map::new();
        for p in self.params {
            match (out.remove(p.name), p.default) {
                (Some(v), _) => {
                    bound.insert(p.name.to_string(), v);
                }
                (None, Some(d)) => {
                    let v = serde_json::from_str(d).unwrap_or(Value::Null);
                    bound.insert(p.name.to_string(), v);
                }
                (None, None) => missing.push(format!("'{}'", p.name)),
            }
        }
        if !missing.is_empty() {
            let names = match missing.len() {
                1 => missing[0].clone(),
                n => format!("{} and {}", missing[..n - 1].join(", "), missing[n - 1]),
            };
            return Err(format!(
                "{short}() missing {} required positional argument{}: {names}",
                missing.len(),
                if missing.len() == 1 { "" } else { "s" }
            ));
        }
        Ok(bound)
    }
}

/// What a running task sees.
#[derive(Clone)]
pub struct TaskCtx {
    /// The pool, Redis and settings (the same state the API builds).
    pub state: AppState,
    /// The job id (Celery's task id).
    pub id: String,
    /// The Celery name.
    pub name: &'static str,
    /// The bound arguments, by Python parameter name.
    pub args: Map<String, Value>,
    /// `self.request.retries`: how many times this call was retried before.
    pub retries: u32,
}

impl TaskCtx {
    /// `AsyncSessionLocal()`: a fresh session (lazy transaction; dropped without commit = rolled back, as
    /// leaving `async with AsyncSessionLocal()` without `commit()` does).
    pub fn session(&self) -> DbSession {
        DbSession::new(self.state.db.clone())
    }

    /// The bound value of a parameter (`Value::Null` if the name is not a parameter).
    pub fn arg(&self, name: &str) -> &Value {
        self.args.get(name).unwrap_or(&Value::Null)
    }

    /// A `str` parameter (a non-string is used as its JSON text, as Python would use whatever was sent).
    pub fn str_arg(&self, name: &str) -> String {
        match self.arg(name) {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }

    /// A `str | None` parameter.
    pub fn opt_str(&self, name: &str) -> Option<String> {
        match self.arg(name) {
            Value::Null => None,
            Value::String(s) => Some(s.clone()),
            other => Some(other.to_string()),
        }
    }

    /// Python truthiness of a parameter (`if late:`).
    pub fn truthy(&self, name: &str) -> bool {
        match self.arg(name) {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
            Value::String(s) => !s.is_empty(),
            Value::Array(a) => !a.is_empty(),
            Value::Object(o) => !o.is_empty(),
        }
    }
}

/// The registered tasks by name.
#[derive(Clone, Default)]
pub struct Registry {
    by_name: HashMap<&'static str, TaskDef>,
}

impl Registry {
    pub fn new(defs: impl IntoIterator<Item = TaskDef>) -> Registry {
        let mut by_name = HashMap::new();
        for d in defs {
            assert!(
                by_name.insert(d.name, d).is_none(),
                "task {} registered twice",
                d.name
            );
        }
        Registry { by_name }
    }

    pub fn get(&self, name: &str) -> Option<&TaskDef> {
        self.by_name.get(name)
    }

    pub fn names(&self) -> Vec<&'static str> {
        let mut v: Vec<_> = self.by_name.keys().copied().collect();
        v.sort_unstable();
        v
    }

    pub fn defs(&self) -> impl Iterator<Item = &TaskDef> {
        self.by_name.values()
    }
}

/// Every task of backend/app/tasks.py ported so far (crate::tasks).
pub fn default_registry() -> Registry {
    Registry::new(crate::tasks::all())
}
