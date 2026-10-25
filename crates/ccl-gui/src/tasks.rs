use crate::{GuiError, GuiResult, account_service::DeviceAuthorization};
use ccl_core::{CancellationToken, TaskContext, TaskEvent};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::VecDeque,
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const LOG_LINES: usize = 1000;
const DISPLAY_LOG_LINES: usize = 100;
const LOG_LINE_CHARS: usize = 512;
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Running,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
}
#[derive(Debug, Clone, Serialize)]
pub struct TaskSnapshot {
    pub id: u64,
    pub revision: u64,
    pub state: TaskState,
    pub kind: String,
    pub cancellable: bool,
    pub phase: String,
    pub completed: u64,
    pub total: Option<u64>,
    pub pid: Option<u32>,
    pub logs: Vec<String>,
    pub error: Option<String>,
    pub error_code: Option<String>,
    pub result: Option<Value>,
    pub authorization: Option<DeviceAuthorization>,
}
impl TaskSnapshot {
    /// Bound log history sent to the WebView while retaining the full native
    /// history for diagnostics and task-history queries inside the process.
    pub fn for_display(mut self) -> Self {
        if self.logs.len() > DISPLAY_LOG_LINES {
            let excess = self.logs.len() - DISPLAY_LOG_LINES;
            self.logs.drain(..excess);
        }
        self
    }
}
struct Record {
    snapshot: TaskSnapshot,
    token: CancellationToken,
    last_publish: Instant,
    dirty: bool,
}
#[derive(Default)]
struct Records {
    current: Option<Record>,
    history: VecDeque<TaskSnapshot>,
    next_id: u64,
}
type Notify = Arc<dyn Fn(TaskSnapshot) + Send + Sync>;
#[derive(Clone)]
pub struct TaskRegistry {
    records: Arc<Mutex<Records>>,
    visible: Arc<AtomicBool>,
    notify: Notify,
}
#[derive(Clone)]
pub struct TaskHandle {
    registry: TaskRegistry,
    id: u64,
    pub context: TaskContext,
}
impl TaskHandle {
    pub fn authorization(&self, authorization: DeviceAuthorization) {
        self.registry
            .edit(self.id, true, |s| s.authorization = Some(authorization));
    }
}
impl TaskRegistry {
    pub fn new(notify: Notify) -> Self {
        Self {
            records: Arc::new(Mutex::new(Records::default())),
            visible: Arc::new(AtomicBool::new(true)),
            notify,
        }
    }
    pub fn snapshot(&self) -> Option<TaskSnapshot> {
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .current
            .as_ref()
            .map(|r| r.snapshot.clone())
    }
    pub fn history(&self) -> Vec<TaskSnapshot> {
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .history
            .iter()
            .cloned()
            .collect()
    }
    pub fn active(&self) -> bool {
        self.snapshot()
            .is_some_and(|s| matches!(s.state, TaskState::Running | TaskState::Cancelling))
    }
    pub fn set_visible(&self, visible: bool) {
        if self.visible.swap(visible, Ordering::AcqRel) != visible && visible {
            self.publish();
        }
    }
    fn publish(&self) {
        if self.visible.load(Ordering::Acquire)
            && let Some(snapshot) = self.snapshot()
        {
            self.publish_snapshot(snapshot);
        }
    }
    fn publish_snapshot(&self, snapshot: TaskSnapshot) {
        if self.visible.load(Ordering::Acquire) {
            (self.notify)(snapshot.for_display());
        }
    }
    fn edit(&self, id: u64, force: bool, edit: impl FnOnce(&mut TaskSnapshot)) {
        let publication = {
            let mut guard = self.records.lock().unwrap_or_else(|e| e.into_inner());
            let Some(record) = guard.current.as_mut().filter(|r| r.snapshot.id == id) else {
                return;
            };
            edit(&mut record.snapshot);
            record.snapshot.revision += 1;
            record.dirty = true;
            let publish = force || record.last_publish.elapsed() >= Duration::from_millis(100);
            if publish {
                record.last_publish = Instant::now();
                record.dirty = false;
            }
            publish.then(|| record.snapshot.clone())
        };
        if let Some(snapshot) = publication {
            self.publish_snapshot(snapshot);
        }
    }
    fn flush_pending(&self, id: u64) {
        let snapshot = {
            let mut guard = self.records.lock().unwrap_or_else(|e| e.into_inner());
            let Some(record) = guard
                .current
                .as_mut()
                .filter(|r| r.snapshot.id == id && r.dirty)
            else {
                return;
            };
            if record.last_publish.elapsed() < Duration::from_millis(100) {
                return;
            }
            record.dirty = false;
            record.last_publish = Instant::now();
            record.snapshot.clone()
        };
        self.publish_snapshot(snapshot);
    }
    pub fn cancel(&self, id: u64) -> GuiResult<TaskSnapshot> {
        let snapshot = {
            let mut guard = self.records.lock().unwrap_or_else(|e| e.into_inner());
            let r = guard
                .current
                .as_mut()
                .filter(|r| r.snapshot.id == id)
                .ok_or_else(|| GuiError::new("task_not_found", "Task no longer exists"))?;
            if r.snapshot.state == TaskState::Running && !r.snapshot.cancellable {
                return Err(GuiError::new(
                    "task_not_cancellable",
                    "This filesystem transaction must finish before closing",
                ));
            }
            if r.snapshot.state == TaskState::Running {
                r.snapshot.state = TaskState::Cancelling;
                r.snapshot.revision += 1;
                r.token.cancel();
                r.dirty = false;
                r.last_publish = Instant::now();
            }
            r.snapshot.clone()
        };
        self.publish_snapshot(snapshot.clone());
        Ok(snapshot)
    }
    /// The owned worker survives view changes; only explicit cancellation stops it.
    pub fn start<F, Fut>(&self, kind: &str, work: F) -> GuiResult<TaskSnapshot>
    where
        F: FnOnce(TaskHandle) -> Fut + Send + 'static,
        Fut: Future<Output = GuiResult<Value>> + Send + 'static,
    {
        self.start_with_cancellation(kind, true, work)
    }
    pub fn start_with_cancellation<F, Fut>(
        &self,
        kind: &str,
        cancellable: bool,
        work: F,
    ) -> GuiResult<TaskSnapshot>
    where
        F: FnOnce(TaskHandle) -> Fut + Send + 'static,
        Fut: Future<Output = GuiResult<Value>> + Send + 'static,
    {
        let initial;
        let token = CancellationToken::default();
        {
            let mut guard = self.records.lock().unwrap_or_else(|e| e.into_inner());
            if guard.current.as_ref().is_some_and(|r| {
                matches!(r.snapshot.state, TaskState::Running | TaskState::Cancelling)
            }) {
                return Err(GuiError::new("task_busy", "Another task is still running"));
            }
            guard.next_id += 1;
            initial = TaskSnapshot {
                id: guard.next_id,
                revision: 1,
                state: TaskState::Running,
                kind: kind.into(),
                cancellable,
                phase: kind.into(),
                completed: 0,
                total: None,
                pid: None,
                logs: vec![],
                error: None,
                error_code: None,
                result: None,
                authorization: None,
            };
            guard.current = Some(Record {
                snapshot: initial.clone(),
                token: token.clone(),
                last_publish: Instant::now(),
                dirty: false,
            });
        }
        let id = initial.id;
        let sink = self.clone();
        let context = TaskContext::new(
            token.clone(),
            Arc::new(move |event| {
                let lifecycle = matches!(
                    &event,
                    TaskEvent::Started { .. }
                        | TaskEvent::ProcessStarted { .. }
                        | TaskEvent::ProcessExited { .. }
                        | TaskEvent::Finished { .. }
                );
                sink.edit(id, lifecycle, |s| match event {
                    TaskEvent::Started { task } => {
                        s.phase = task;
                        s.completed = 0;
                        s.total = None;
                    }
                    TaskEvent::Progress {
                        task,
                        completed,
                        total,
                    } => {
                        s.phase = task;
                        s.completed = completed;
                        s.total = total;
                    }
                    TaskEvent::ProcessStarted { pid } => {
                        s.pid = Some(pid);
                        s.phase = "game_running".into();
                    }
                    TaskEvent::Log { line, .. } => {
                        if s.logs.len() == LOG_LINES {
                            s.logs.remove(0);
                        }
                        s.logs.push(line.chars().take(LOG_LINE_CHARS).collect());
                    }
                    TaskEvent::ProcessExited { .. } => s.phase = "game_exited".into(),
                    TaskEvent::Finished { task } => s.phase = task,
                });
            }),
        );
        let registry = self.clone();
        let handle = TaskHandle {
            registry: self.clone(),
            id,
            context,
        };
        let spawn = std::thread::Builder::new()
            .name(format!("ccl-task-{id}"))
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let runtime = tokio::runtime::Builder::new_multi_thread()
                        .worker_threads(2)
                        .enable_all()
                        .build()
                        .map_err(|_| GuiError::new("worker_error", "Cannot create task runtime"))?;
                    let flush_registry = registry.clone();
                    runtime.block_on(async move {
                        // Flush the final coalesced log/progress batch even when the
                        // producer becomes idle. This timer exists only during work.
                        let flush = tokio::spawn(async move {
                            loop {
                                tokio::time::sleep(Duration::from_millis(100)).await;
                                flush_registry.flush_pending(id);
                            }
                        });
                        let result = work(handle).await;
                        flush.abort();
                        result
                    })
                }))
                .unwrap_or_else(|_| {
                    Err(GuiError::new(
                        "worker_error",
                        "Task worker stopped unexpectedly",
                    ))
                });
                registry.finish(id, result);
            });
        if spawn.is_err() {
            self.finish(
                id,
                Err(GuiError::new("worker_error", "Cannot start task worker")),
            );
        }
        Ok(initial)
    }

    fn finish(&self, id: u64, result: GuiResult<Value>) {
        let snapshot = {
            let mut guard = self.records.lock().unwrap_or_else(|e| e.into_inner());
            let Some(record) = guard.current.as_mut().filter(|r| r.snapshot.id == id) else {
                return;
            };
            let s = &mut record.snapshot;
            match result {
                Ok(value) => {
                    if value.pointer("/launch/cancelled").and_then(Value::as_bool) == Some(true) {
                        s.state = TaskState::Cancelled;
                    } else if value.get("launch").is_some()
                        && value.pointer("/launch/exit_code").and_then(Value::as_i64) != Some(0)
                    {
                        s.state = TaskState::Failed;
                        s.error_code = Some("game_exit".into());
                        s.error = Some(
                            match value.pointer("/launch/exit_code").and_then(Value::as_i64) {
                                Some(code) => format!("Game exited with status code {code}."),
                                None => "Game terminated without a normal exit status.".into(),
                            },
                        );
                    } else {
                        s.state = TaskState::Succeeded;
                    }
                    s.result = Some(value);
                }
                Err(error) => {
                    s.state = if error.code == "cancelled" {
                        TaskState::Cancelled
                    } else {
                        TaskState::Failed
                    };
                    s.error_code = Some(error.code.into());
                    s.error = Some(error.message);
                }
            }
            s.authorization = None;
            s.revision += 1;
            record.last_publish = Instant::now();
            record.dirty = false;
            let snapshot = s.clone();
            if guard.history.len() == 8 {
                guard.history.pop_front();
            }
            guard.history.push_back(snapshot.clone());
            snapshot
        };
        self.publish_snapshot(snapshot);
    }
    #[cfg(test)]
    pub fn start_probe(&self) -> GuiResult<TaskSnapshot> {
        self.probe(100_000, Duration::from_millis(6))
    }
    #[cfg(test)]
    fn probe(&self, total: u64, interval: Duration) -> GuiResult<TaskSnapshot> {
        self.start("probe", move |handle| async move {
            for completed in 1..=total {
                handle
                    .context
                    .emit(TaskEvent::Log {
                        stream: "fixture".into(),
                        line: format!("[Synthetic task fixture] bounded log line {completed}"),
                    })
                    .map_err(|_| GuiError::new("cancelled", "Probe cancelled"))?;
                handle
                    .context
                    .emit(TaskEvent::Progress {
                        task: "synthetic_probe".into(),
                        completed,
                        total: Some(total),
                    })
                    .map_err(|_| GuiError::new("cancelled", "Probe cancelled"))?;
                if !interval.is_zero() {
                    tokio::time::sleep(interval).await;
                }
            }
            Ok(serde_json::json!({"synthetic":true}))
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn wait(registry: &TaskRegistry) -> TaskSnapshot {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let s = registry.snapshot().unwrap();
            if !matches!(s.state, TaskState::Running | TaskState::Cancelling) {
                return s;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    #[test]
    fn cancellation_reaches_terminal_state_even_when_core_stops_emitting() {
        let r = TaskRegistry::new(Arc::new(|_| {}));
        let s = r.start_probe().unwrap();
        assert!(r.start_probe().is_err());
        r.cancel(s.id).unwrap();
        assert_eq!(wait(&r).state, TaskState::Cancelled);
    }
    #[test]
    fn hidden_task_retains_bounded_logs_and_resumes_notifications() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = calls.clone();
        let r = TaskRegistry::new(Arc::new(move |_| {
            count.fetch_add(1, Ordering::Relaxed);
        }));
        r.set_visible(false);
        r.probe(3000, Duration::ZERO).unwrap();
        let s = wait(&r);
        assert_eq!(s.state, TaskState::Succeeded);
        assert_eq!(s.logs.len(), LOG_LINES);
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        r.set_visible(true);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }
    #[test]
    fn raw_history_keeps_1000_lines_while_notifications_keep_the_latest_100_and_terminal_state() {
        let notifications = Arc::new(Mutex::new(Vec::new()));
        let received = notifications.clone();
        let registry = TaskRegistry::new(Arc::new(move |snapshot| {
            received.lock().unwrap().push(snapshot);
        }));
        registry
            .start("log_fixture", |handle| async move {
                for index in 1..=1200 {
                    handle
                        .context
                        .emit(TaskEvent::Log {
                            stream: "fixture".into(),
                            line: format!("log-{index:04}"),
                        })
                        .unwrap();
                }
                Ok(serde_json::json!({"complete":true}))
            })
            .unwrap();

        let raw = wait(&registry);
        assert_eq!(raw.state, TaskState::Succeeded);
        assert_eq!(raw.logs.len(), LOG_LINES);
        assert_eq!(raw.logs.first().map(String::as_str), Some("log-0201"));
        assert_eq!(raw.logs.last().map(String::as_str), Some("log-1200"));

        let display = raw.for_display();
        assert_eq!(display.logs.len(), DISPLAY_LOG_LINES);
        assert_eq!(display.logs.first().map(String::as_str), Some("log-1101"));
        assert_eq!(display.logs.last().map(String::as_str), Some("log-1200"));
        assert_eq!(display.state, TaskState::Succeeded);

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let terminal_notified = notifications
                .lock()
                .unwrap()
                .last()
                .is_some_and(|snapshot| snapshot.state == TaskState::Succeeded);
            if terminal_notified {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "terminal notification was not published"
            );
            std::thread::yield_now();
        }
        let sent = notifications.lock().unwrap();
        assert!(!sent.is_empty());
        assert!(
            sent.iter()
                .all(|snapshot| snapshot.logs.len() <= DISPLAY_LOG_LINES)
        );
        let terminal = sent.last().unwrap();
        assert_eq!(terminal.state, TaskState::Succeeded);
        assert_eq!(terminal.logs.first().map(String::as_str), Some("log-1101"));
        assert_eq!(terminal.logs.last().map(String::as_str), Some("log-1200"));
    }
    #[test]
    fn unknown_total_pid_result_and_worker_failure_are_reported() {
        let r = TaskRegistry::new(Arc::new(|_| {}));
        r.start("launch", |h| async move {
            h.context
                .emit(TaskEvent::ProcessStarted { pid: 42 })
                .unwrap();
            Ok(serde_json::json!({"launch":{"cancelled":false}}))
        })
        .unwrap();
        let s = wait(&r);
        assert_eq!(s.pid, Some(42));
        assert_eq!(s.total, None);
        assert!(s.result.is_some());
        r.start("failure", |_| async {
            Err(GuiError::new("fixture", "Safe error"))
        })
        .unwrap();
        assert_eq!(wait(&r).error_code.as_deref(), Some("fixture"));
    }
}
