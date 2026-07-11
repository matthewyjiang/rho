use tokio::sync::mpsc;

const MAX_EVENTS_PER_FRAME: usize = 256;

pub(super) fn handle_batch<T, E>(
    first: T,
    receiver: &mut mpsc::UnboundedReceiver<T>,
    mut handle: impl FnMut(T) -> Result<(), E>,
) -> Result<(), E> {
    handle(first)?;
    for _ in 1..MAX_EVENTS_PER_FRAME {
        let Ok(event) = receiver.try_recv() else {
            break;
        };
        handle(event)?;
    }
    Ok(())
}
