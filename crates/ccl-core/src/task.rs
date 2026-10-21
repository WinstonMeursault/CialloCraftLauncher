use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use serde::Serialize;

use crate::{Error, Result};

/// Progress events produced by core operations.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaskEvent {
    Started {
        task: String,
    },
    Progress {
        task: String,
        completed: u64,
        total: Option<u64>,
    },
    Finished {
        task: String,
    },
}

/// Cooperative cancellation handle that can be shared across tasks.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Event consumer supplied by CLI, tests, or a future graphical application.
pub type EventSink = Arc<dyn Fn(TaskEvent) + Send + Sync>;

#[derive(Clone)]
pub struct TaskContext {
    pub cancellation: CancellationToken,
    sink: EventSink,
}

impl TaskContext {
    pub fn new(cancellation: CancellationToken, sink: EventSink) -> Self {
        Self { cancellation, sink }
    }

    pub fn emit(&self, event: TaskEvent) -> Result<()> {
        self.cancellation.check()?;
        (self.sink)(event);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn cancellation_stops_event_delivery() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let output = Arc::clone(&received);
        let token = CancellationToken::default();
        let context = TaskContext::new(
            token.clone(),
            Arc::new(move |event| output.lock().unwrap().push(event)),
        );
        context
            .emit(TaskEvent::Started {
                task: "install".into(),
            })
            .unwrap();
        token.cancel();
        assert!(matches!(
            context.emit(TaskEvent::Finished {
                task: "install".into()
            }),
            Err(Error::Cancelled)
        ));
        assert_eq!(received.lock().unwrap().len(), 1);
    }
}
