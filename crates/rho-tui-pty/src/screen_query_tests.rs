use super::ScreenModel;
use pretty_assertions::assert_eq;

// Covers: cursor queries must use the cursor at the query, not at the end of a
// read chunk, and must survive every chunk boundary. Owner: terminal emulation.
#[test]
fn cursor_queries_reply_at_the_requested_position_across_chunks() {
    let output = b"\x1b[2;3H\x1b[6n\x1b[4;5H\x1b[6n";
    for split in 0..=output.len() {
        let mut screen = ScreenModel::new(8, 12);
        screen.process(&output[..split]);
        screen.process(&output[split..]);
        assert_eq!(screen.take_terminal_replies(), b"\x1b[2;3R\x1b[4;5R");
        assert_eq!(screen.cursor(), (3, 4));
        assert_eq!(screen.take_terminal_replies(), Vec::<u8>::new());
    }
}
