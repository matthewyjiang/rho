use std::io::{Seek as _, SeekFrom};

use super::*;

// Covers: shebang inspection must not change later reads, including on parse errors.
// Owner: secure filesystem executable inspection.
#[test]
fn raw_shebang_preserves_the_open_file_offset() {
    for (bytes, expects_error) in [
        (b"#!/bin/sh\nexit 0".as_slice(), false),
        (b"#!\xff\nexit 0".as_slice(), true),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("script");
        std::fs::write(&path, bytes).unwrap();
        let mut file = File::open(path).unwrap();
        file.seek(SeekFrom::Start(4)).unwrap();

        let result = raw_shebang(&file);

        assert_eq!(result.is_err(), expects_error);
        assert_eq!(file.stream_position().unwrap(), 4);
    }
}
