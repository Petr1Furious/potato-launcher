use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use instance::storage::InstanceHandle;
use launcher_bridge::{InstanceTaskView, ProgressUnit, TaskKind};
use tokio::sync::mpsc;
use utils::progress::ProgressTracker;

use crate::BackendEvent;

/// Minimum interval between `inc`-driven snapshot emissions; structural
/// changes (new task, set_length, set_message, finish) always emit so the
/// final state of every task is guaranteed to reach the frontend.
const INC_EMIT_INTERVAL: Duration = Duration::from_millis(33);

struct Inner {
    tasks: Vec<InstanceTaskView>,
    next_id: u64,
    last_emit: Option<Instant>,
}

/// Live progress rows for one instance activity (install or launch prep).
/// Worker code appends task rows and updates them through [`TaskHandle`];
/// every update sends a throttled full snapshot to the backend event loop.
#[derive(Clone)]
pub(crate) struct InstanceTaskList {
    handle: InstanceHandle,
    internal: mpsc::UnboundedSender<BackendEvent>,
    inner: Arc<Mutex<Inner>>,
}

impl InstanceTaskList {
    pub fn new(handle: InstanceHandle, internal: mpsc::UnboundedSender<BackendEvent>) -> Self {
        Self {
            handle,
            internal,
            inner: Arc::new(Mutex::new(Inner {
                tasks: Vec::new(),
                next_id: 0,
                last_emit: None,
            })),
        }
    }

    pub fn task(
        &self,
        kind: TaskKind,
        unit: ProgressUnit,
        message: impl Into<Arc<str>>,
    ) -> TaskHandle {
        let id = {
            let mut inner = self.inner.lock().expect("task list poisoned");
            let id = inner.next_id;
            inner.next_id += 1;
            inner.tasks.push(InstanceTaskView {
                id,
                kind,
                message: message.into(),
                current: 0,
                total: 0,
                unit,
                finished: false,
            });
            self.emit_locked(&mut inner);
            id
        };
        TaskHandle {
            list: self.clone(),
            id,
        }
    }

    pub fn snapshot(&self) -> Arc<[InstanceTaskView]> {
        let inner = self.inner.lock().expect("task list poisoned");
        inner.tasks.clone().into()
    }

    fn update(&self, id: u64, force: bool, apply: impl FnOnce(&mut InstanceTaskView)) {
        let mut inner = self.inner.lock().expect("task list poisoned");
        let Some(task) = inner.tasks.iter_mut().find(|task| task.id == id) else {
            return;
        };
        apply(task);
        let throttled = !force
            && inner
                .last_emit
                .is_some_and(|last| last.elapsed() < INC_EMIT_INTERVAL);
        if !throttled {
            self.emit_locked(&mut inner);
        }
    }

    fn emit_locked(&self, inner: &mut Inner) {
        inner.last_emit = Some(Instant::now());
        let _ = self.internal.send(BackendEvent::InstanceTasks {
            handle: self.handle.clone(),
            tasks: inner.tasks.clone().into(),
        });
    }
}

#[derive(Clone)]
pub(crate) struct TaskHandle {
    list: InstanceTaskList,
    id: u64,
}

impl TaskHandle {
    pub fn set_message(&self, message: impl Into<Arc<str>>) {
        let message = message.into();
        self.list
            .update(self.id, true, |task| task.message = message);
    }
}

impl ProgressTracker for TaskHandle {
    fn set_length(&self, length: u64) {
        self.list.update(self.id, true, |task| {
            task.total = length;
            task.current = 0;
            task.finished = false;
        });
    }

    fn inc(&self, amount: u64) {
        self.list.update(self.id, false, |task| {
            task.current = task.current.saturating_add(amount);
        });
    }

    fn dec(&self, amount: u64) {
        self.list.update(self.id, false, |task| {
            task.current = task.current.saturating_sub(amount);
        });
    }

    fn set_message(&self, message: &str) {
        TaskHandle::set_message(self, message);
    }

    fn finish(&self) {
        self.list.update(self.id, true, |task| {
            if task.total > 0 && task.current > task.total {
                task.total = task.current;
            }
            task.finished = true;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task_list() -> (
        InstanceTaskList,
        mpsc::UnboundedReceiver<BackendEvent>,
        InstanceHandle,
    ) {
        let (tx, rx) = mpsc::unbounded_channel();
        let handle = InstanceHandle::from("test:instance");
        (InstanceTaskList::new(handle.clone(), tx), rx, handle)
    }

    fn latest_tasks(rx: &mut mpsc::UnboundedReceiver<BackendEvent>) -> Arc<[InstanceTaskView]> {
        let mut latest = None;
        while let Ok(event) = rx.try_recv() {
            let BackendEvent::InstanceTasks { tasks, .. } = event else {
                panic!("unexpected event");
            };
            latest = Some(tasks);
        }
        latest.expect("no task snapshot emitted")
    }

    #[test]
    fn tasks_keep_creation_order_and_ids() {
        let (list, mut rx, _) = task_list();
        list.task(TaskKind::Metadata, ProgressUnit::Items, "meta");
        list.task(TaskKind::Download, ProgressUnit::Bytes, "files");
        list.task(TaskKind::Java, ProgressUnit::Bytes, "java");

        let tasks = latest_tasks(&mut rx);
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0].kind, TaskKind::Metadata);
        assert_eq!(tasks[1].kind, TaskKind::Download);
        assert_eq!(tasks[2].kind, TaskKind::Java);
        assert!(tasks.iter().enumerate().all(|(i, t)| t.id == i as u64));
    }

    #[test]
    fn inc_is_throttled_but_finish_always_emits() {
        let (list, mut rx, _) = task_list();
        let task = list.task(TaskKind::Download, ProgressUnit::Items, "files");
        task.set_length(1000);
        while rx.try_recv().is_ok() {}

        // immediately after the set_length emission, incs are throttled
        for _ in 0..100 {
            task.inc(1);
        }
        assert!(rx.try_recv().is_err());

        task.finish();
        let tasks = latest_tasks(&mut rx);
        assert_eq!(tasks[0].current, 100);
        assert!(tasks[0].finished);
    }

    #[test]
    fn finish_clamps_total_to_current_overflow() {
        let (list, mut rx, _) = task_list();
        let task = list.task(TaskKind::Download, ProgressUnit::Bytes, "files");
        task.set_length(10);
        task.inc(25);
        task.finish();

        let tasks = latest_tasks(&mut rx);
        assert_eq!(tasks[0].current, 25);
        assert_eq!(tasks[0].total, 25);
    }

    #[test]
    fn snapshot_reflects_all_updates_regardless_of_throttle() {
        let (list, _rx, _) = task_list();
        let task = list.task(TaskKind::CheckFiles, ProgressUnit::Items, "check");
        task.set_length(10);
        task.inc(3);
        task.dec(1);

        let tasks = list.snapshot();
        assert_eq!(tasks[0].current, 2);
        assert_eq!(tasks[0].total, 10);
    }
}
