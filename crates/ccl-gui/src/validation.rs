//! Opt-in GUI performance fixtures. This module is excluded from normal builds.
use crate::{GuiError, GuiResult, TaskRegistry, TaskSnapshot};
use ccl_core::TaskEvent;
use serde_json::json;
use std::time::Duration;

const LOG_COUNT: u64 = 100_000;
const LINES_PER_BATCH: u64 = 100;
const BATCH_INTERVAL: Duration = Duration::from_millis(600);
const BATCH_COUNT: u64 = LOG_COUNT / LINES_PER_BATCH;

/// Start a fixed 100,000-line / 10-minute stream through the real task event path.
/// The task remains cancellable through the ordinary task UI.
pub fn start_log_pressure(registry: &TaskRegistry) -> GuiResult<TaskSnapshot> {
    registry.start("gui_log_pressure", move |handle| async move {
        for batch in 0..BATCH_COUNT {
            for offset in 0..LINES_PER_BATCH {
                let line = batch * LINES_PER_BATCH + offset + 1;
                handle.context.emit(TaskEvent::Log {
                    stream: "gui_validation".into(),
                    line: format!("[GUI performance fixture] {line:06} steady local line for bounded task log rendering"),
                }).map_err(|_| GuiError::new("cancelled", "GUI log pressure fixture cancelled"))?;
            }
            handle
                .context
                .emit(TaskEvent::Progress {
                    task: "gui_log_pressure".into(),
                    completed: batch + 1,
                    total: Some(BATCH_COUNT),
                })
                .map_err(|_| GuiError::new("cancelled", "GUI log pressure fixture cancelled"))?;
            tokio::time::sleep(BATCH_INTERVAL).await;
        }
        Ok(json!({"validation_fixture":"gui_log_pressure","lines":LOG_COUNT,"lines_per_minute":10_000,"lines_per_batch":LINES_PER_BATCH,"batch_interval_ms":BATCH_INTERVAL.as_millis(),"planned_duration_seconds":600}))
    })
}
