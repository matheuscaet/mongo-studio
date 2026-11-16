//! Calls that outlive a client's patience. Many MCP clients give up on a
//! request after about a minute, while a call may wait longer for the user
//! to approve it, or run longer than that. So every call runs on its own
//! task: when it ends within `Inner::inline_wait` the client gets its
//! result as usual; otherwise it gets a task id at once, with what the call
//! is doing (waiting for the user, or running and what it printed so far),
//! and follows it with `get_task` or stops it with `cancel_task`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::watch;
use tokio::task::AbortHandle;
use uuid::Uuid;

use super::tools::ToolOutput;
use super::Inner;
use crate::mongosh::describe_ms;

/// A finished task is kept this long for `get_task`.
const KEEP_FINISHED: Duration = Duration::from_secs(30 * 60);
/// How many printed lines a status shows at most.
const STATUS_LINES: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Checking the arguments and the policy.
    Starting,
    /// Waiting for the user to approve it in the app.
    Approval,
    Running,
    Done,
}

struct State {
    phase: Phase,
    printed: Vec<String>,
    output: Option<ToolOutput>,
    finished_at: Option<Instant>,
}

/// One call, followed while it runs.
pub(crate) struct Task {
    pub id: String,
    pub client_id: String,
    pub tool: &'static str,
    /// Whether the tool can change data (so running it again would repeat
    /// the change).
    pub changes_data: bool,
    started: Instant,
    state: Mutex<State>,
    done: watch::Sender<bool>,
    abort: Mutex<Option<AbortHandle>>,
}

impl Task {
    pub fn new(client_id: &str, tool: &'static str, changes_data: bool) -> Arc<Self> {
        Arc::new(Self {
            id: Uuid::new_v4().to_string(),
            client_id: client_id.to_string(),
            tool,
            changes_data,
            started: Instant::now(),
            state: Mutex::new(State {
                phase: Phase::Starting,
                printed: Vec::new(),
                output: None,
                finished_at: None,
            }),
            done: watch::channel(false).0,
            abort: Mutex::new(None),
        })
    }

    pub fn set_phase(&self, phase: Phase) {
        self.state.lock().unwrap().phase = phase;
    }

    pub fn phase(&self) -> Phase {
        self.state.lock().unwrap().phase
    }

    /// A line the call printed (a script's print/console.log).
    pub fn print(&self, line: String) {
        self.state.lock().unwrap().printed.push(line);
    }

    pub fn printed_count(&self) -> usize {
        self.state.lock().unwrap().printed.len()
    }

    pub fn finish(&self, output: ToolOutput) {
        {
            let mut state = self.state.lock().unwrap();
            state.phase = Phase::Done;
            state.output = Some(output);
            state.finished_at = Some(Instant::now());
        }
        self.done.send_replace(true);
    }

    pub fn output(&self) -> Option<ToolOutput> {
        self.state.lock().unwrap().output.clone()
    }

    pub fn set_abort(&self, handle: AbortHandle) {
        *self.abort.lock().unwrap() = Some(handle);
    }

    /// Waits up to `wait` for the call to end; its output when it did.
    pub async fn wait(&self, wait: Duration) -> Option<ToolOutput> {
        let mut done = self.done.subscribe();
        let _ = tokio::time::timeout(wait, done.wait_for(|d| *d)).await;
        self.output()
    }

    /// Stops the call: a pending approval is withdrawn, a running call is
    /// dropped, which stops a script and the query it waits on. Returns
    /// false when it had already ended.
    pub fn cancel(&self) -> bool {
        if self.phase() == Phase::Done {
            return false;
        }
        if let Some(handle) = self.abort.lock().unwrap().take() {
            handle.abort();
        }
        self.finish(ToolOutput {
            text: "Cancelled. Changes made before that were kept.".to_string(),
            is_error: true,
        });
        true
    }

    fn elapsed(&self) -> String {
        describe_ms(self.started.elapsed().as_secs() * 1000)
    }

    /// What the client is told while the call goes on.
    pub fn status_text(&self, from_line: usize) -> String {
        let state = self.state.lock().unwrap();
        let id = &self.id;
        let again = if self.changes_data {
            format!(
                "Don't call {} again: it would run a second time.",
                self.tool
            )
        } else {
            format!("No need to call {} again.", self.tool)
        };
        let mut text = match state.phase {
            Phase::Starting | Phase::Approval => format!(
                "Waiting for the user to approve this in Mongo Studio: the request is shown \
                 there, and nothing has run yet. Tell the user to look at Mongo Studio. Then call \
                 get_task with task_id \"{id}\" to wait for the outcome; cancel_task withdraws \
                 the request. {again}"
            ),
            Phase::Running | Phase::Done => format!(
                "Still running after {}, in the background (task_id \"{id}\"). Call get_task \
                 with this task_id to wait for the result and see what it prints; cancel_task \
                 stops it. {again}",
                self.elapsed()
            ),
        };
        let total = state.printed.len();
        let from = from_line.min(total);
        if total > 0 {
            let shown_from = from.max(total.saturating_sub(STATUS_LINES));
            text.push_str(&format!(
                "\nPrinted so far: {total} line{}.",
                if total == 1 { "" } else { "s" }
            ));
            if shown_from < total {
                text.push_str(&format!(
                    " Lines {}-{total} (pass from_line={total} next time for the new ones):\n{}",
                    shown_from + 1,
                    state.printed[shown_from..].join("\n")
                ));
            }
        }
        text
    }

    fn expired(&self) -> bool {
        let state = self.state.lock().unwrap();
        state
            .finished_at
            .is_some_and(|at| at.elapsed() >= KEEP_FINISHED)
    }
}

#[derive(Default)]
pub(crate) struct Tasks {
    tasks: Mutex<HashMap<String, Arc<Task>>>,
}

impl Tasks {
    pub fn add(&self, task: Arc<Task>) {
        let mut tasks = self.tasks.lock().unwrap();
        tasks.retain(|_, t| !t.expired());
        tasks.insert(task.id.clone(), task);
    }

    pub fn get(&self, id: &str) -> Option<Arc<Task>> {
        let mut tasks = self.tasks.lock().unwrap();
        tasks.retain(|_, t| !t.expired());
        tasks.get(id.trim()).cloned()
    }

    /// Stops the calls of a client that went away.
    pub fn cancel_client(&self, client_id: &str) {
        let tasks: Vec<Arc<Task>> = self
            .tasks
            .lock()
            .unwrap()
            .values()
            .filter(|t| t.client_id == client_id)
            .cloned()
            .collect();
        for task in tasks {
            task.cancel();
        }
    }

    pub fn cancel_all(&self) {
        let tasks: Vec<Arc<Task>> = self.tasks.lock().unwrap().drain().map(|(_, t)| t).collect();
        for task in tasks {
            task.cancel();
        }
    }
}

/// Runs `work` as `task`, and answers within `inner.inline_wait`: with the
/// output when it ended by then, else with the task's status.
pub(crate) async fn run<F>(inner: &Arc<Inner>, task: Arc<Task>, work: F) -> ToolOutput
where
    F: std::future::Future<Output = ToolOutput> + Send + 'static,
{
    let handle = tokio::spawn(work);
    task.set_abort(handle.abort_handle());
    let running = task.clone();
    tokio::spawn(async move {
        let output = match handle.await {
            Ok(output) => output,
            // Cancelled: the task already holds its answer.
            Err(e) if e.is_cancelled() => return,
            Err(_) => {
                log::error!("MCP server: {} failed with an internal error", running.tool);
                ToolOutput {
                    text: "Mongo Studio hit an internal error running this call.".to_string(),
                    is_error: true,
                }
            }
        };
        // A cancelled task already holds its answer.
        if running.phase() != Phase::Done {
            running.finish(output);
        }
    });
    if let Some(output) = task.wait(inner.inline_wait).await {
        return output;
    }
    inner.tasks.add(task.clone());
    ToolOutput {
        text: task.status_text(0),
        is_error: false,
    }
}

/// `get_task`: waits up to `wait` for the task, then reports.
pub(crate) async fn get(inner: &Inner, id: &str, wait: Duration, from_line: usize) -> ToolOutput {
    let Some(task) = inner.tasks.get(id) else {
        return unknown(id);
    };
    match task.wait(wait).await {
        Some(output) => {
            let printed = task.printed_count();
            let mut text = format!(
                "Task {} ({}) {} after {}.",
                task.id,
                task.tool,
                if output.is_error {
                    "failed"
                } else {
                    "finished"
                },
                task.elapsed()
            );
            if printed > 0 && !output.text.contains("Printed") {
                text.push_str(&format!(" It printed {printed} lines."));
            }
            ToolOutput {
                text: format!("{text}\n{}", output.text),
                is_error: output.is_error,
            }
        }
        None => ToolOutput {
            text: task.status_text(from_line),
            is_error: false,
        },
    }
}

/// `cancel_task`.
pub(crate) fn cancel(inner: &Inner, id: &str) -> ToolOutput {
    let Some(task) = inner.tasks.get(id) else {
        return unknown(id);
    };
    let was = task.phase();
    if !task.cancel() {
        return ToolOutput {
            text: format!(
                "Task {} had already ended; call get_task for its result.",
                task.id
            ),
            is_error: false,
        };
    }
    let text = match was {
        Phase::Starting | Phase::Approval => {
            "Withdrew the request from Mongo Studio; nothing ran.".to_string()
        }
        _ => format!(
            "Stopped {} after {}. Changes it made before that were kept.",
            task.tool,
            task.elapsed()
        ),
    };
    ToolOutput {
        text,
        is_error: false,
    }
}

fn unknown(id: &str) -> ToolOutput {
    ToolOutput {
        text: format!(
            "No task has the id \"{}\": it may have ended more than {} min ago, or the server \
             restarted.",
            id.trim(),
            KEEP_FINISHED.as_secs() / 60
        ),
        is_error: true,
    }
}
