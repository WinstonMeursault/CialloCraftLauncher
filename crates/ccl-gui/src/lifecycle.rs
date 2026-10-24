//! Coordinate application quit with task starts and synchronous mutations.
use crate::TaskRegistry;
use std::sync::{
    Mutex, TryLockError,
    atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitDecision {
    Ready,
    TaskRunning,
    OperationRunning,
}

/// Idle quit closes the same admission gate used by IPC task starts. A busy
/// application must route through the window's cancellation/wait confirmation.
pub fn begin_exit(gate: &Mutex<()>, closing: &AtomicBool, tasks: &TaskRegistry) -> ExitDecision {
    let _guard = match gate.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::Poisoned(error)) => error.into_inner(),
        Err(TryLockError::WouldBlock) => return ExitDecision::OperationRunning,
    };
    if tasks.active() {
        return ExitDecision::TaskRunning;
    }
    closing.store(true, Ordering::Release);
    ExitDecision::Ready
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };
    #[test]
    fn idle_quit_blocks_new_ipc_admission_before_releasing_gate() {
        let tasks = TaskRegistry::new(Arc::new(|_| {}));
        let gate = Mutex::new(());
        let closing = AtomicBool::new(false);
        assert_eq!(begin_exit(&gate, &closing, &tasks), ExitDecision::Ready);
        let _guard = gate.try_lock().unwrap();
        assert!(closing.load(Ordering::Acquire));
    }
    #[test]
    fn quit_does_not_interrupt_an_ongoing_metadata_or_vault_mutation() {
        let tasks = TaskRegistry::new(Arc::new(|_| {}));
        let gate = Mutex::new(());
        let closing = AtomicBool::new(false);
        let guard = gate.lock().unwrap();
        assert_eq!(
            begin_exit(&gate, &closing, &tasks),
            ExitDecision::OperationRunning
        );
        assert!(!closing.load(Ordering::Acquire));
        drop(guard);
        assert_eq!(begin_exit(&gate, &closing, &tasks), ExitDecision::Ready);
    }
    #[test]
    fn active_game_or_install_requires_confirmation_then_idle_exit() {
        let tasks = TaskRegistry::new(Arc::new(|_| {}));
        let gate = Mutex::new(());
        let closing = AtomicBool::new(false);
        let task = tasks
            .start("launch", |handle| async move {
                while !handle.context.cancellation.is_cancelled() {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
                Err(crate::GuiError::new("cancelled", "Stopped fixture"))
            })
            .unwrap();
        assert_eq!(
            begin_exit(&gate, &closing, &tasks),
            ExitDecision::TaskRunning
        );
        assert!(!closing.load(Ordering::Acquire));
        tasks.cancel(task.id).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while tasks.active() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(begin_exit(&gate, &closing, &tasks), ExitDecision::Ready);
    }
}
