use std::{
    io::{self, Write},
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};

use pretty_assertions::assert_eq;

use super::{access_with_notice, NoticeState};

/// Shared capture so the test can read what the watcher thread wrote. Each
/// flush is reported on `flushed`, letting a "slow" access block until the
/// notice is out instead of sleeping.
#[derive(Clone)]
struct Output {
    bytes: Arc<Mutex<Vec<u8>>>,
    flushed: mpsc::Sender<()>,
}

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = self.flushed.send(());
        Ok(())
    }
}

fn leaked_notice(state: NoticeState) -> &'static Mutex<NoticeState> {
    Box::leak(Box::new(Mutex::new(state)))
}

// Covers: a quick keyring call prints nothing, a call still running after the
// delay prints the hint once, and nothing prints after TUI takeover disables it.
// Owner: application credential startup notification policy. No desktop keyring
// is needed to exercise this policy, unlike an interactive unlock-dialog test.
#[test]
fn notice_prints_once_only_for_slow_startup_keyring_access() {
    let notice = leaked_notice(NoticeState::Pending);
    let (flushed, flushes) = mpsc::channel();
    let output = Output {
        bytes: Arc::default(),
        flushed,
    };
    let never = Duration::from_secs(3600);

    assert_eq!(access_with_notice(notice, never, output.clone(), || 1), 1);
    assert_eq!(output.bytes.lock().unwrap().len(), 0);

    // Blocks until the watcher flushes, standing in for an unlock dialog.
    let slow = || flushes.recv().unwrap();
    access_with_notice(notice, Duration::ZERO, output.clone(), slow);
    let first_notice = output.bytes.lock().unwrap().clone();
    assert!(!first_notice.is_empty());

    access_with_notice(notice, Duration::ZERO, output.clone(), || ());
    *notice.lock().unwrap() = NoticeState::Disabled;
    access_with_notice(notice, Duration::ZERO, output.clone(), || ());
    assert_eq!(*output.bytes.lock().unwrap(), first_notice);
    assert!(flushes.try_recv().is_err());
}
