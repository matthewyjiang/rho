use super::*;

#[test]
fn drains_only_complete_newline_terminated_lines() {
    let mut stream = AppendOnlyStream::default();

    stream.push_delta("hel");
    assert_eq!(stream.drain_renderable(10), None);

    stream.push_delta("lo\nwor");
    let fragment = stream.drain_renderable(10).unwrap();
    assert_eq!(fragment.text.as_str(), "hello\n");
    assert!(fragment.include_leading_blank());
    assert_eq!(stream.emitted_text(), "hello\n");

    stream.push_delta("ld\n");
    let fragment = stream.drain_renderable(10).unwrap();
    assert_eq!(fragment.text.as_str(), "world\n");
    assert!(!fragment.include_leading_blank());
    assert_eq!(stream.emitted_text(), "hello\nworld\n");
}

#[test]
fn drains_full_width_wrapped_visual_lines() {
    let mut stream = AppendOnlyStream::default();

    stream.push_delta("abcdefg");
    let fragment = stream.drain_renderable(3).unwrap();
    assert_eq!(fragment.text.as_str(), "abcdef");
    assert_eq!(stream.emitted_text(), "abcdef");

    assert_eq!(stream.drain_renderable(3), None);
    let fragment = stream.finish().unwrap();
    assert_eq!(fragment.text.as_str(), "g");
    assert_eq!(stream.emitted_text(), "abcdefg");
}

#[test]
fn exact_width_text_keeps_trailing_space_pending_until_finish() {
    let mut stream = AppendOnlyStream::default();

    stream.push_delta("abc ");
    let fragment = stream.drain_renderable(3).unwrap();
    assert_eq!(fragment.text.as_str(), "abc");
    assert_eq!(fragment.render_text(), "abc");

    let fragment = stream.finish().unwrap();
    assert_eq!(fragment.text.as_str(), " ");
    assert_eq!(fragment.render_text(), " ");
    assert_eq!(stream.emitted_text(), "abc ");
}

#[test]
fn newline_after_full_width_wrap_updates_text_without_extra_visual_line() {
    let mut stream = AppendOnlyStream::default();

    stream.push_delta("abc");
    let fragment = stream.drain_renderable(3).unwrap();
    assert_eq!(fragment.text.as_str(), "abc");
    assert_eq!(fragment.render_text(), "abc");

    stream.push_delta("\n");
    let fragment = stream.drain_renderable(3).unwrap();
    assert_eq!(fragment.text.as_str(), "\n");
    assert_eq!(fragment.render_text(), "");
    assert_eq!(stream.emitted_text(), "abc\n");
}

#[test]
fn second_newline_after_full_width_wrap_renders_blank_line() {
    let mut stream = AppendOnlyStream::default();

    stream.push_delta("abc");
    assert_eq!(stream.drain_renderable(3).unwrap().render_text(), "abc");

    stream.push_delta("\n\n");
    let fragment = stream.drain_renderable(3).unwrap();
    assert_eq!(fragment.text.as_str(), "\n\n");
    assert_eq!(fragment.render_text(), "\n");
    assert_eq!(stream.emitted_text(), "abc\n\n");
}

#[test]
fn preserves_blank_lines_and_multibyte_boundaries() {
    let mut stream = AppendOnlyStream::default();

    stream.push_delta("é\n\n");
    let fragment = stream.drain_renderable(5).unwrap();
    assert_eq!(fragment.text.as_str(), "é\n\n");
    assert_eq!(stream.emitted_text(), "é\n\n");
}

#[test]
fn reset_clears_pending_emitted_and_leading_blank_state() {
    let mut stream = AppendOnlyStream::default();
    stream.push_delta("done\n");
    assert!(stream.drain_renderable(10).unwrap().include_leading_blank());

    stream.reset();
    stream.push_delta("again\n");
    let fragment = stream.drain_renderable(10).unwrap();
    assert_eq!(fragment.text.as_str(), "again\n");
    assert!(fragment.include_leading_blank());
    assert_eq!(stream.emitted_text(), "again\n");
}

#[test]
fn returned_fragments_are_append_only() {
    let mut stream = AppendOnlyStream::default();
    let mut rendered = String::new();

    for delta in ["ab", "c\n", "de", "f", "\ng"] {
        stream.push_delta(delta);
        if let Some(fragment) = stream.drain_renderable(10) {
            rendered.push_str(fragment.text.as_str());
        }
    }

    assert_eq!(rendered, "abc\ndef\n");
    assert_eq!(stream.finish().unwrap().text.as_str(), "g");
    assert_eq!(stream.emitted_text(), "abc\ndef\ng");
}
