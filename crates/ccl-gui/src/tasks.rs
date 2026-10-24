use crate::{GuiError, GuiResult};
use ccl_core::{CancellationToken, TaskContext, TaskEvent};
use serde::Serialize;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const LOG_LINES: usize = 1000;
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
    pub completed: u64,
    pub total: u64,
    pub logs: Vec<String>,
    pub error: Option<String>,
}
struct Record {
    snapshot: TaskSnapshot,
    logs: VecDeque<String>,
    token: CancellationToken,
}
type Notify = Arc<dyn Fn(TaskSnapshot) + Send + Sync>;
#[derive(Clone)]
pub struct TaskRegistry {
    record: Arc<Mutex<Option<Record>>>,
    visible: Arc<AtomicBool>,
    notify: Notify,
}
impl TaskRegistry {
    pub fn new(notify: Notify) -> Self {
        Self {
            record: Arc::new(Mutex::new(None)),
            visible: Arc::new(AtomicBool::new(true)),
            notify,
        }
    }
    pub fn snapshot(&self) -> Option<TaskSnapshot> {
        self.record
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|r| {
                let mut snapshot = r.snapshot.clone();
                snapshot.logs = r.logs.iter().cloned().collect();
                snapshot
            })
    }
    pub fn set_visible(&self, visible: bool) {
        if visible && !self.visible.swap(visible, Ordering::AcqRel) {
            self.publish();
        } else if !visible {
            self.visible.store(false, Ordering::Release);
        }
    }
    pub fn active(&self) -> bool {
        self.snapshot()
            .is_some_and(|s| matches!(s.state, TaskState::Running | TaskState::Cancelling))
    }
    fn publish(&self) {
        if self.visible.load(Ordering::Acquire)
            && let Some(s) = self.snapshot()
        {
            (self.notify)(s);
        }
    }
    pub fn cancel(&self, id: u64) -> GuiResult<TaskSnapshot> {
        {
            let mut guard = self.record.lock().unwrap_or_else(|e| e.into_inner());
            let record = guard
                .as_mut()
                .filter(|r| r.snapshot.id == id)
                .ok_or_else(|| GuiError::new("task_not_found", "Task no longer exists"))?;
            if record.snapshot.state == TaskState::Running {
                record.snapshot.state = TaskState::Cancelling;
                record.snapshot.revision += 1;
                record.token.cancel();
            }
        }
        self.publish();
        self.snapshot()
            .ok_or_else(|| GuiError::new("task_not_found", "Task no longer exists"))
    }
    pub fn start_probe(&self) -> GuiResult<TaskSnapshot> {
        self.start(100_000, Duration::from_millis(6))
    }
    fn start(&self, total: u64, interval: Duration) -> GuiResult<TaskSnapshot> {
        let initial;
        let token = CancellationToken::default();
        {
            let mut guard = self.record.lock().unwrap_or_else(|e| e.into_inner());
            if guard.as_ref().is_some_and(|r| {
                matches!(r.snapshot.state, TaskState::Running | TaskState::Cancelling)
            }) {
                return Err(GuiError::new("task_busy", "A probe is already running"));
            }
            let id = guard.as_ref().map_or(1, |r| r.snapshot.id + 1);
            initial = TaskSnapshot {
                id,
                revision: 1,
                state: TaskState::Running,
                completed: 0,
                total,
                logs: vec![],
                error: None,
            };
            *guard = Some(Record {
                snapshot: initial.clone(),
                logs: VecDeque::new(),
                token: token.clone(),
            });
        }
        let registry = self.clone();
        let sink_registry = self.clone();
        let context = TaskContext::new(
            token,
            Arc::new(move |event| {
                let mut guard = sink_registry
                    .record
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(record) = guard.as_mut() {
                    match event {
                        TaskEvent::Progress { completed, .. } => {
                            record.snapshot.completed = completed
                        }
                        TaskEvent::Log { line, .. } => {
                            if record.logs.len() == LOG_LINES {
                                record.logs.pop_front();
                            }
                            record
                                .logs
                                .push_back(line.chars().take(LOG_LINE_CHARS).collect());
                        }
                        _ => (),
                    }
                    record.snapshot.revision += 1;
                }
            }),
        );
        std::thread::spawn(move || {
            let mut last = Instant::now();
            let result = (|| {
                for completed in 1..=total {
                    context.emit(TaskEvent::Log {
                        stream: "fixture".into(),
                        line: format!("[G0 synthetic fixture] bounded log line {completed}"),
                    })?;
                    context.emit(TaskEvent::Progress {
                        task: "g0_synthetic_probe".into(),
                        completed,
                        total: Some(total),
                    })?;
                    if last.elapsed() >= Duration::from_millis(100) {
                        registry.publish();
                        last = Instant::now();
                    }
                    std::thread::sleep(interval);
                }
                Ok::<_, ccl_core::Error>(())
            })();
            {
                let mut guard = registry.record.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(record) = guard.as_mut() {
                    record.snapshot.state = match result {
                        Ok(()) => TaskState::Succeeded,
                        Err(ccl_core::Error::Cancelled) => TaskState::Cancelled,
                        Err(_) => TaskState::Failed,
                    };
                    record.snapshot.revision += 1;
                    if record.snapshot.state == TaskState::Failed {
                        record.snapshot.error = Some("Core task fixture failed".into());
                    }
                }
            }
            registry.publish();
        });
        Ok(initial)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wait(registry: &TaskRegistry) -> TaskSnapshot {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let snapshot = registry.snapshot().unwrap();
            if !registry.active() {
                return snapshot;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn cancellation_reaches_terminal_state_even_when_core_stops_emitting() {
        let registry = TaskRegistry::new(Arc::new(|_| {}));
        let initial = registry.start_probe().unwrap();
        assert!(registry.start_probe().is_err());
        registry.cancel(initial.id).unwrap();
        let final_state = wait(&registry);
        assert_eq!(final_state.state, TaskState::Cancelled);
        assert!(final_state.revision > initial.revision);
    }
    #[test]
    fn hidden_probe_has_no_notifications_and_retains_bounded_snapshot() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = calls.clone();
        let registry = TaskRegistry::new(Arc::new(move |_| {
            count.fetch_add(1, Ordering::Relaxed);
        }));
        registry.set_visible(false);
        registry.start(3000, Duration::ZERO).unwrap();
        let snapshot = wait(&registry);
        assert_eq!(snapshot.state, TaskState::Succeeded);
        assert_eq!(snapshot.logs.len(), LOG_LINES);
        assert_eq!(snapshot.completed, 3000);
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        registry.set_visible(true);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(registry.snapshot().unwrap().state, TaskState::Succeeded);
    }
}
