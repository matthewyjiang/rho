use super::*;

/// Requests host input while no synchronous tool batch or provider call is active.
/// Acceptance and history checkpointing are synchronous after the reply, so a host
/// can commit its reservation without racing event backpressure or cancellation.
pub(super) async fn collect(
    runtime: &Rho,
    core: &SessionCore,
    run_id: &RunId,
    boundary: crate::InputBoundary,
    history: &mut Vec<Message>,
    cancellation: &CancellationToken,
    events: &mpsc::Sender<RunEvent>,
) -> Result<bool, Error> {
    let session_id = core.id();
    let Some(source) = &runtime.boundary_inputs else {
        return Ok(false);
    };
    let reply = tokio::select! {
        biased;
        () = cancellation.cancelled() => return Err(Error::Cancelled),
        () = events.closed() => return Err(Error::Interrupted { message: "run event consumer disconnected".into(), source: None }),
        reply = source.request(session_id, run_id, boundary) => reply?,
    };
    let input = reply.input;
    if let Some(input) = &input {
        // Typed so readers never mistake host input for human steering;
        // providers still receive the host-framed blocks as user content.
        history.push(Message::BoundaryInput(input.blocks().into()));
        // Acknowledgement releases host reservations. Checkpoint first so an
        // event-consumer disconnect or dropped Run cannot discard accepted
        // findings with the rest of an uncommitted working history.
        core.commit(history.clone())?;
    }
    let _ = reply.accepted.send(());
    let Some(input) = input else { return Ok(false) };
    emit(
        events,
        cancellation,
        RunEvent::BoundaryInputApplied {
            session_id: session_id.clone(),
            run_id: run_id.clone(),
            input,
        },
    )
    .await?;
    Ok(true)
}
