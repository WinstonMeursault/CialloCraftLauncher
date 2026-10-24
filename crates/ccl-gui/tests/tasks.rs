use ccl_core::TaskEvent;
use ccl_gui::{GuiError, TaskRegistry, TaskSnapshot, TaskState};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

fn wait_terminal(registry: &TaskRegistry) -> TaskSnapshot {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = registry.snapshot().expect("task snapshot");
        if !matches!(snapshot.state, TaskState::Running | TaskState::Cancelling) {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "task did not reach terminal state"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn finishing_task_keeps_its_snapshot_with_its_history_id_across_immediate_starts() {
    let registry = TaskRegistry::new(Arc::new(|_| {}));
    let mut finished = Vec::new();
    for n in 0..40u64 {
        let initial = registry
            .start("stress", move |_| async move { Ok(json!({"sequence": n})) })
            .unwrap();
        let terminal = wait_terminal(&registry);
        assert_eq!(terminal.id, initial.id);
        assert_eq!(terminal.state, TaskState::Succeeded);
        assert_eq!(terminal.result.as_ref().unwrap()["sequence"], n);
        finished.push((terminal.id, n));
    }

    let history = registry.history();
    assert_eq!(history.len(), 8);
    for (snapshot, (expected_id, expected_sequence)) in history.iter().zip(&finished[32..]) {
        assert_eq!(snapshot.id, *expected_id);
        assert_eq!(
            snapshot.result.as_ref().unwrap()["sequence"],
            *expected_sequence
        );
        assert_eq!(snapshot.state, TaskState::Succeeded);
    }
}

#[test]
fn cancellation_and_panics_always_create_terminal_history() {
    let registry = TaskRegistry::new(Arc::new(|_| {}));
    let started = registry
        .start("cancel", |handle| async move {
            while !handle.context.cancellation.is_cancelled() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            Err(GuiError::new("cancelled", "Task cancelled"))
        })
        .unwrap();
    registry.cancel(started.id).unwrap();
    let cancelled = wait_terminal(&registry);
    assert_eq!(cancelled.state, TaskState::Cancelled);
    assert_eq!(registry.history().last().unwrap().id, started.id);

    registry
        .start("panic", |_| async move { panic!("fixture worker panic") })
        .unwrap();
    let panicked = wait_terminal(&registry);
    assert_eq!(panicked.state, TaskState::Failed);
    assert_eq!(panicked.error_code.as_deref(), Some("worker_error"));
    assert_eq!(registry.history().last().unwrap().id, panicked.id);
}

#[test]
fn launch_exit_status_is_a_safe_failure_and_cancellation_takes_precedence() {
    let registry = TaskRegistry::new(Arc::new(|_| {}));
    registry
        .start("launch", |_| async {
            Ok(json!({"launch":{"exit_code": 17, "cancelled": false}}))
        })
        .unwrap();
    let failed = wait_terminal(&registry);
    assert_eq!(failed.state, TaskState::Failed);
    assert_eq!(failed.error_code.as_deref(), Some("game_exit"));
    assert_eq!(
        failed.error.as_deref(),
        Some("Game exited with status code 17.")
    );

    registry
        .start("launch", |_| async {
            Ok(json!({"launch":{"exit_code": 17, "cancelled": true}}))
        })
        .unwrap();
    assert_eq!(wait_terminal(&registry).state, TaskState::Cancelled);
    registry
        .start("launch", |_| async {
            Ok(json!({"launch":{"exit_code": null, "cancelled": false}}))
        })
        .unwrap();
    assert_eq!(wait_terminal(&registry).state, TaskState::Failed);
}

#[test]
fn progress_notifications_are_throttled_and_unknown_totals_and_logs_are_bounded() {
    let notifications = Arc::new(Mutex::new(Vec::<(Instant, TaskSnapshot)>::new()));
    let observed = notifications.clone();
    let registry = TaskRegistry::new(Arc::new(move |snapshot| {
        observed.lock().unwrap().push((Instant::now(), snapshot));
    }));
    registry
        .start("fixture", |handle| async move {
            handle
                .context
                .emit(TaskEvent::Started {
                    task: "prepare".into(),
                })
                .unwrap();
            for n in 0..1200u64 {
                handle
                    .context
                    .emit(TaskEvent::Progress {
                        task: "download".into(),
                        completed: n,
                        total: None,
                    })
                    .unwrap();
                handle
                    .context
                    .emit(TaskEvent::Log {
                        stream: "stdout".into(),
                        line: format!("{n}:{}", "x".repeat(700)),
                    })
                    .unwrap();
            }
            Ok(Value::Null)
        })
        .unwrap();
    let terminal = wait_terminal(&registry);
    assert_eq!(terminal.state, TaskState::Succeeded);
    assert_eq!(terminal.total, None);
    assert_eq!(terminal.completed, 1199);
    assert_eq!(terminal.logs.len(), 1000);
    assert!(terminal.logs.last().unwrap().len() <= 512);
    assert!(terminal.logs.last().unwrap().starts_with("1199:"));

    let notices = notifications.lock().unwrap();
    // Started and terminal updates are immediate. Progress and logs share a 10 Hz cap.
    assert!(
        notices.len() <= 5,
        "unexpected notification burst: {}",
        notices.len()
    );
    assert_eq!(notices.last().unwrap().1.state, TaskState::Succeeded);
}

#[test]
fn active_task_rejects_a_second_start_and_survives_visibility_changes() {
    let registry = TaskRegistry::new(Arc::new(|_| {}));
    let started = registry
        .start("held", |_| async {
            tokio::time::sleep(Duration::from_millis(40)).await;
            Ok(Value::Null)
        })
        .unwrap();
    assert_eq!(
        registry
            .start("second", |_| async { Ok(Value::Null) })
            .unwrap_err()
            .code,
        "task_busy"
    );
    registry.set_visible(false);
    assert_eq!(registry.snapshot().unwrap().id, started.id);
    assert_eq!(wait_terminal(&registry).state, TaskState::Succeeded);
}

#[test]
fn single_log_is_delivered_while_producer_becomes_idle() {
    let (sender, receiver) = std::sync::mpsc::channel();
    let registry = TaskRegistry::new(Arc::new(move |snapshot| {
        let _ = sender.send(snapshot);
    }));
    let task = registry
        .start("launch", |handle| async move {
            handle
                .context
                .emit(TaskEvent::Log {
                    stream: "stdout".into(),
                    line: "one real-time line".into(),
                })
                .unwrap();
            while !handle.context.cancellation.is_cancelled() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(GuiError::new("cancelled", "Stopped fixture"))
        })
        .unwrap();
    let published = receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("the last throttled log batch must be flushed without another producer event");
    assert_eq!(published.state, TaskState::Running);
    assert_eq!(published.logs, vec!["one real-time line"]);
    registry.cancel(task.id).unwrap();
    assert_eq!(wait_terminal(&registry).state, TaskState::Cancelled);
}
