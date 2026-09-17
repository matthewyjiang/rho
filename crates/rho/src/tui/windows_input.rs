//! Windows VT input for crates.io builds. Crossterm 0.29 discards the raw
//! control characters before its public event stream, so decoding must happen
//! at the console-record boundary rather than above EventStream.
use crossterm::event::Event;
use std::{
    fs::File,
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    sync::Arc,
    thread,
};
use tokio::sync::mpsc;
use windows_sys::Win32::{
    Foundation::{WAIT_FAILED, WAIT_OBJECT_0},
    System::{
        Console::{
            GetNumberOfConsoleInputEvents, ReadConsoleInputW, ENABLE_VIRTUAL_TERMINAL_INPUT,
            INPUT_RECORD,
        },
        Threading::{CreateEventW, SetEvent, WaitForMultipleObjects, INFINITE},
    },
};

#[path = "windows_input_mode.rs"]
mod mode;
pub(in crate::tui) use mode::PasteMode;
#[path = "windows_input_records.rs"]
mod records;

pub(super) struct Input {
    events: mpsc::UnboundedReceiver<io::Result<Event>>,
    stop: Arc<OwnedHandle>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Input {
    /// `None` selects the legacy reader only when no VT transport is active.
    pub(super) fn start() -> io::Result<Option<Self>> {
        let console = match mode::console() {
            Ok(console) => console,
            // Crossterm supplies the legacy error/fallback for missing consoles.
            Err(_) => return Ok(None),
        };
        if mode::mode(&console)? & ENABLE_VIRTUAL_TERMINAL_INPUT == 0 {
            return Ok(None);
        }
        let handle = unsafe {
            CreateEventW(
                std::ptr::null(),
                /*manual_reset*/ 1,
                /*initial_state*/ 0,
                std::ptr::null(),
            )
        };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // The event remains live until both reader and owner have released it.
        let stop = Arc::new(unsafe { OwnedHandle::from_raw_handle(handle) });
        let worker_stop = Arc::clone(&stop);
        let (tx, events) = mpsc::unbounded_channel();
        let worker = thread::Builder::new()
            .name("rho-windows-input".into())
            .spawn(move || {
                if let Err(error) = read(console, &worker_stop, &tx) {
                    let _ = tx.send(Err(error));
                }
            })?;
        Ok(Some(Self {
            events,
            stop,
            worker: Some(worker),
        }))
    }

    pub(super) async fn next(&mut self) -> io::Result<Event> {
        super::event_result(self.events.recv().await)
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        // Wake the native wait, then join before handing the console to a shell
        // or another reader. Cancelling next() alone never consumes an event.
        unsafe {
            SetEvent(self.stop.as_raw_handle());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn read(
    console: File,
    stop: &OwnedHandle,
    tx: &mpsc::UnboundedSender<io::Result<Event>>,
) -> io::Result<()> {
    let handles = [stop.as_raw_handle(), console.as_raw_handle()];
    let mut decoder = records::Decoder::default();
    loop {
        let ready = unsafe {
            WaitForMultipleObjects(
                handles.len() as u32,
                handles.as_ptr(),
                /*wait_all*/ 0,
                INFINITE,
            )
        };
        if ready == WAIT_OBJECT_0 {
            return Ok(());
        }
        if ready == WAIT_FAILED {
            return Err(io::Error::last_os_error());
        }
        if ready != WAIT_OBJECT_0 + 1 {
            return Err(io::Error::other("unexpected console wait result"));
        }
        let mut count = 0;
        if unsafe { GetNumberOfConsoleInputEvents(console.as_raw_handle(), &mut count) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if count == 0 {
            continue;
        }
        // This reader exclusively owns CONIN$ while active. Allocate from the
        // actual queue length rather than imposing a paste-size or batch cap.
        let mut records = vec![INPUT_RECORD::default(); count as usize];
        let mut read = 0;
        if unsafe {
            ReadConsoleInputW(
                console.as_raw_handle(),
                records.as_mut_ptr(),
                count,
                &mut read,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if unsafe { GetNumberOfConsoleInputEvents(console.as_raw_handle(), &mut count) } == 0 {
            return Err(io::Error::last_os_error());
        }
        for event in decoder.decode(&records[..read as usize], /*more*/ count != 0) {
            if tx.send(Ok(event)).is_err() {
                return Ok(());
            }
        }
    }
}
