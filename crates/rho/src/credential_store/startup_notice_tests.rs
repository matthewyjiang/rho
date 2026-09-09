use std::io::{self, Write};

use pretty_assertions::assert_eq;

use super::{CredentialStoreBackend, NoticeState};

#[derive(Default)]
struct Output {
    bytes: Vec<u8>,
    flushes: usize,
}

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flushes += 1;
        Ok(())
    }
}

// Covers: the hint must be flushed before a blocking keyring call, once only,
// and must not leak into file-backed startup or output after TUI takeover.
// Owner: application credential startup notification policy. No desktop keyring
// is needed to exercise this policy, unlike an interactive unlock-dialog test.
#[test]
fn notice_is_flushed_once_and_only_for_startup_keyring_access() {
    let mut state = NoticeState::Pending;
    let mut output = Output::default();
    state.before_access(CredentialStoreBackend::File, &mut output);
    assert_eq!((output.bytes.len(), output.flushes), (0, 0));

    state.before_access(CredentialStoreBackend::Os, &mut output);
    assert!(!output.bytes.is_empty());
    assert_eq!(output.flushes, 1);
    let first_notice = output.bytes.clone();

    state.before_access(CredentialStoreBackend::Os, &mut output);
    assert_eq!((&output.bytes, output.flushes), (&first_notice, 1));

    state = NoticeState::Disabled;
    state.before_access(CredentialStoreBackend::Os, &mut output);
    assert_eq!((output.bytes, output.flushes), (first_notice, 1));
}
