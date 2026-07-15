/// Incrementally decodes LF- or CRLF-terminated UTF-8 lines without moving the
/// unprocessed buffer after every line.
///
/// Complete lines borrow the decoder's buffer. Before appending another chunk,
/// the decoder compacts at most the unconsumed tail from the previous chunk.
#[derive(Default)]
pub(crate) struct LineDecoder {
    buffer: Vec<u8>,
    start: usize,
}

impl LineDecoder {
    pub(crate) fn push(&mut self, chunk: &[u8]) {
        self.compact();
        self.buffer.extend_from_slice(chunk);
    }

    pub(crate) fn next_line(&mut self) -> Result<Option<&str>, std::str::Utf8Error> {
        let Some(relative_end) = self.buffer[self.start..]
            .iter()
            .position(|byte| *byte == b'\n')
        else {
            return Ok(None);
        };
        let end = self.start + relative_end;
        let line_end = end - usize::from(end > self.start && self.buffer[end - 1] == b'\r');
        let line = std::str::from_utf8(&self.buffer[self.start..line_end])?;
        self.start = end + 1;
        Ok(Some(line))
    }

    pub(crate) fn finish(&mut self) -> Result<Option<&str>, std::str::Utf8Error> {
        if self.start == self.buffer.len() {
            return Ok(None);
        }
        let line_end =
            self.buffer.len() - usize::from(self.buffer.last().is_some_and(|byte| *byte == b'\r'));
        let line = std::str::from_utf8(&self.buffer[self.start..line_end])?;
        self.start = self.buffer.len();
        Ok(Some(line))
    }

    fn compact(&mut self) {
        if self.start == 0 {
            return;
        }
        if self.start == self.buffer.len() {
            self.buffer.clear();
        } else {
            self.buffer.copy_within(self.start.., 0);
            self.buffer.truncate(self.buffer.len() - self.start);
        }
        self.start = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::LineDecoder;

    #[test]
    fn decodes_lf_crlf_empty_lines_and_trailing_tail_across_chunks() {
        let mut decoder = LineDecoder::default();
        let mut lines = Vec::new();

        for chunk in [
            &b"first\r"[..],
            &b"\nsecond\n\nmultibyte: \xc3"[..],
            &b"\xa9\r\ntail\r"[..],
        ] {
            decoder.push(chunk);
            while let Some(line) = decoder.next_line().unwrap() {
                lines.push(line.to_string());
            }
        }
        if let Some(line) = decoder.finish().unwrap() {
            lines.push(line.to_string());
        }

        assert_eq!(lines, ["first", "second", "", "multibyte: é", "tail"]);
    }

    #[test]
    fn retains_only_an_incomplete_line_when_appending_a_chunk() {
        let mut decoder = LineDecoder::default();
        decoder.push(b"one\ntwo\npar");
        assert_eq!(decoder.next_line().unwrap(), Some("one"));
        assert_eq!(decoder.next_line().unwrap(), Some("two"));
        assert_eq!(decoder.next_line().unwrap(), None);

        decoder.push(b"tial\n");

        assert_eq!(decoder.next_line().unwrap(), Some("partial"));
        assert_eq!(decoder.finish().unwrap(), None);
    }

    #[test]
    fn waits_for_a_complete_multibyte_character() {
        let mut decoder = LineDecoder::default();
        decoder.push(b"data: \xc3");
        assert_eq!(decoder.next_line().unwrap(), None);

        decoder.push(b"\xa9\n");

        assert_eq!(decoder.next_line().unwrap(), Some("data: é"));
    }

    #[test]
    fn rejects_invalid_utf8_in_complete_lines_and_tail() {
        let mut line_decoder = LineDecoder::default();
        line_decoder.push(b"data: \xff\n");
        assert!(line_decoder.next_line().is_err());

        let mut tail_decoder = LineDecoder::default();
        tail_decoder.push(b"data: \xff");
        assert!(tail_decoder.finish().is_err());
    }
}
