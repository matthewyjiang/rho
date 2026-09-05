use super::*;
use pretty_assertions::assert_eq;

// Covers: a terminal snapshot returned at poll timeout acknowledges the exit,
// including when restoring an earlier automatic-delivery reservation afterward.
// Owner: process manager. A manual clock forces the otherwise narrow timeout race.
#[tokio::test(start_paused = true)]
async fn terminal_poll_timeout_acknowledgement_survives_reservation_restore() {
    let (manager, record) = manager_with_record(State::Running);
    let wait = Duration::from_secs(1); // Manual-clock interval, not a wall-clock synchronization delay.
    let poll = manager.poll_bounded("fixture", None, wait, usize::MAX);
    tokio::pin!(poll);
    assert!(futures_util::poll!(&mut poll).is_pending());
    {
        let mut record = record.lock().unwrap();
        record.state = State::Exited;
        record.exit_code = Some(7);
    }
    let reserved = manager.take_notifications();
    assert_eq!(reserved.len(), 1);
    tokio::time::advance(wait).await;
    let snapshot = poll.await.unwrap();
    assert_eq!(
        (snapshot.state, snapshot.exit_code),
        (State::Exited, Some(7))
    );
    manager.restore_notifications(&reserved);
    assert!(manager.take_notifications().is_empty());
}

// Covers: failed spawn/ProcessTree attachment uses the same handoff gate as
// supervisor exits, so an unseen startup failure remains queued after closing.
// Owner: process terminal publication, independent of process exit polling.
#[test]
fn failed_start_publication_waits_for_snapshot_handoff() {
    let (manager, record) = manager_with_record(State::Starting);
    let publication = Arc::new(std::sync::Barrier::new(2));
    let worker = {
        let _snapshot = crate::app::notification_delivery::lock();
        let worker = {
            let publication = Arc::clone(&publication);
            let record = Arc::clone(&record);
            let exited = Arc::clone(&manager.exited);
            std::thread::spawn(move || {
                publication.wait();
                mark_terminal(
                    &record,
                    State::FailedToStart,
                    Some("fixture spawn failure".into()),
                    &exited,
                );
            })
        };
        publication.wait();
        assert!(manager.take_notifications().is_empty());
        assert_eq!(record.lock().unwrap().state, State::Starting);
        worker
    };
    worker.join().unwrap();
    let notifications = manager.take_notifications();
    assert_eq!(
        notifications
            .iter()
            .map(|notice| (notice.process_id.as_str(), notice.state))
            .collect::<Vec<_>>(),
        vec![("fixture", State::FailedToStart)]
    );
}

fn manager_with_record(state: State) -> (ProcessManager, SharedRecord) {
    let manager = ProcessManager::new(ProcessLimits::default());
    let record = Arc::new(Mutex::new(Record {
        id: "fixture".into(),
        command: "fixture".into(),
        state,
        started: Instant::now(),
        completed: None,
        last_output_at: None,
        chunks: VecDeque::new(),
        bytes: 0,
        next: 0,
        exit_code: None,
        detail: None,
        stop: None,
        tree: None,
        notify: Arc::new(Notify::new()),
        observed: false,
        explicitly_observed: false,
    }));
    manager
        .inner
        .lock()
        .unwrap()
        .records
        .insert("fixture".into(), record.clone());
    (manager, record)
}
