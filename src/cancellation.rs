use tokio::sync::watch;

/// Cooperative cancellation shared by one agent run and its nested operations.
#[derive(Clone, Debug)]
pub struct RunCancellation {
    cancelled: watch::Receiver<bool>,
    cancel: watch::Sender<bool>,
}

impl Default for RunCancellation {
    fn default() -> Self {
        let (cancel, cancelled) = watch::channel(false);
        Self { cancelled, cancel }
    }
}

impl RunCancellation {
    pub fn cancel(&self) {
        self.cancel.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.cancelled.borrow()
    }

    pub async fn cancelled(&self) {
        let mut cancelled = self.cancelled.clone();
        if *cancelled.borrow_and_update() {
            return;
        }
        while cancelled.changed().await.is_ok() {
            if *cancelled.borrow_and_update() {
                return;
            }
        }
    }
}

#[cfg(test)]
#[path = "cancellation_tests.rs"]
mod tests;
