use std::sync::{Arc, LazyLock};

use serde::Serialize;

pub static TASK_STATUS_NOTIFIER: LazyLock<TaskStatusNotifier> = LazyLock::new(TaskStatusNotifier::new);

#[derive(Serialize, Clone, Default)]
pub struct TaskStatus {
    pub is_running: bool,
    pub last_run: Option<chrono::DateTime<chrono::Local>>,
    pub last_finish: Option<chrono::DateTime<chrono::Local>>,
    pub next_run: Option<chrono::DateTime<chrono::Local>>,
}

pub struct TaskStatusNotifier {
    tx: tokio::sync::watch::Sender<Arc<TaskStatus>>,
    rx: tokio::sync::watch::Receiver<Arc<TaskStatus>>,
}

impl TaskStatusNotifier {
    pub fn new() -> Self {
        let (tx, rx) = tokio::sync::watch::channel(Arc::new(TaskStatus::default()));
        Self { tx, rx }
    }

    /// 简单的开始运行方法，不返回锁
    pub fn set_running(&self) {
        let _ = self.tx.send(Arc::new(TaskStatus {
            is_running: true,
            last_run: Some(chrono::Local::now()),
            last_finish: None,
            next_run: None,
        }));
    }

    /// 简单的结束运行方法，不需要锁
    pub fn set_finished(&self) {
        let last_status = self.tx.borrow();
        let last_run = last_status.last_run;
        drop(last_status);

        // 简化实现，使用固定的2小时间隔
        let now = chrono::Local::now();
        let _ = self.tx.send(Arc::new(TaskStatus {
            is_running: false,
            last_run,
            last_finish: Some(now),
            next_run: now.checked_add_signed(chrono::Duration::hours(2)),
        }));
    }

    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<Arc<TaskStatus>> {
        self.rx.clone()
    }
}
