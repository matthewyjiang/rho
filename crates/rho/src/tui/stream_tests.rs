use super::super::markdown::markdown_lines;
use super::*;
use ratatui::text::Line;

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn rendered_markdown_text(text: &str, width: usize, in_code_block: bool) -> Vec<String> {
    let mut in_code_block = in_code_block;
    markdown_lines(text, width, &mut in_code_block)
        .iter()
        .map(line_text)
        .collect()
}

#[test]
fn preview_does_not_drain_pending_or_commit_emitted_text() {
    let mut stream = AppendOnlyStream::default();

    stream.push_delta("hel");
    let preview = stream.drain_preview().unwrap();
    assert_eq!(preview.render_text(), "hel");
    assert_eq!(stream.pending_text(), "hel");
    assert_eq!(stream.emitted_text(), "");

    stream.push_delta("lo\n");
    let fragment = stream.drain_renderable(10).unwrap();
    assert_eq!(fragment.text.as_str(), "hello\n");
    assert_eq!(stream.emitted_text(), "hello\n");
}

#[test]
fn markdown_preview_does_not_emit_unsafe_code_fence_fragment() {
    let mut stream = AppendOnlyStream::default();

    stream.push_delta("```");
    assert_eq!(stream.drain_preview_markdown(10, false), None);

    stream.push_delta("rust\n");
    let fragment = stream.drain_renderable_markdown(10, false).unwrap();
    assert_eq!(fragment.text.as_str(), "```rust\n");
}

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
fn markdown_drain_waits_for_complete_emphasis_before_wrapping() {
    let mut stream = AppendOnlyStream::default();

    stream.push_delta("**hel");
    assert_eq!(stream.drain_renderable_markdown(5, false), None);

    stream.push_delta("lo** ");
    let fragment = stream.drain_renderable_markdown(5, false).unwrap();
    assert_eq!(fragment.text.as_str(), "**hello**");
    assert_eq!(
        rendered_markdown_text(fragment.render_text(), 5, false),
        vec!["hello"]
    );
    assert_eq!(stream.emitted_text(), "**hello**");
}

#[test]
fn markdown_drain_waits_for_complete_link_before_wrapping() {
    let mut stream = AppendOnlyStream::default();

    stream.push_delta("[x]");
    assert_eq!(stream.drain_renderable_markdown(4, false), None);

    stream.push_delta("(y) ");
    let fragment = stream.drain_renderable_markdown(4, false).unwrap();
    assert_eq!(fragment.text.as_str(), "[x](y)");
    assert_eq!(
        rendered_markdown_text(fragment.render_text(), 4, false),
        vec!["x: y"]
    );
    assert_eq!(stream.emitted_text(), "[x](y)");
}

#[test]
fn markdown_drain_waits_for_complete_inline_code_before_wrapping() {
    let mut stream = AppendOnlyStream::default();

    stream.push_delta("`hel");
    assert_eq!(stream.drain_renderable_markdown(5, false), None);

    stream.push_delta("lo` ");
    let fragment = stream.drain_renderable_markdown(5, false).unwrap();
    assert_eq!(fragment.text.as_str(), "`hello`");
    assert_eq!(
        rendered_markdown_text(fragment.render_text(), 5, false),
        vec!["hello"]
    );
    assert_eq!(stream.emitted_text(), "`hello`");
}

#[test]
fn markdown_drain_keeps_streamed_heading_atomic_across_narrow_wraps() {
    let mut stream = AppendOnlyStream::default();
    let source = "## **streamed heading with unicode 你🙂**";

    for ch in source.chars() {
        stream.push_delta(&ch.to_string());
        assert_eq!(stream.drain_renderable_markdown(9, false), None);
    }
    assert_eq!(stream.emitted_text(), "");

    stream.push_delta("\n");
    let fragment = stream.drain_renderable_markdown(9, false).unwrap();
    assert_eq!(fragment.text.as_str(), format!("{source}\n"));

    let mut fragment_code_block = false;
    let fragment_lines = markdown_lines(fragment.render_text(), 9, &mut fragment_code_block);
    let mut final_code_block = false;
    let final_lines = markdown_lines(source, 9, &mut final_code_block);
    assert_eq!(fragment_lines, final_lines);
}

#[test]
fn markdown_drain_resumes_wrapping_once_hash_prefix_is_not_a_heading() {
    let mut stream = AppendOnlyStream::default();

    stream.push_delta("#hashtag text ");
    let fragment = stream.drain_renderable_markdown(8, false).unwrap();

    assert_eq!(fragment.text.as_str(), "#hashtag");
    assert_eq!(
        rendered_markdown_text(fragment.render_text(), 8, false),
        vec!["#hashtag"]
    );
}

#[test]
fn markdown_drain_waits_for_complete_code_fence_line() {
    let mut stream = AppendOnlyStream::default();

    stream.push_delta("```");
    assert_eq!(stream.drain_renderable_markdown(3, false), None);

    stream.push_delta("rust\n");
    let fragment = stream.drain_renderable_markdown(3, false).unwrap();
    assert_eq!(fragment.text.as_str(), "```rust\n");
    assert_eq!(stream.emitted_text(), "```rust\n");
}

#[test]
fn markdown_drain_allows_markers_inside_code_blocks() {
    let mut stream = AppendOnlyStream::default();

    stream.push_delta("__init__");
    let fragment = stream.drain_renderable_markdown(4, true).unwrap();
    assert_eq!(fragment.text.as_str(), "__init__");
    assert_eq!(stream.emitted_text(), "__init__");
}

#[test]
fn markdown_drain_uses_rendered_width_for_trailing_spaces_and_exact_wraps() {
    let mut stream = AppendOnlyStream::default();

    stream.push_delta("abc ");
    let fragment = stream.drain_renderable_markdown(3, false).unwrap();
    assert_eq!(fragment.text.as_str(), "abc");
    assert_eq!(
        rendered_markdown_text(fragment.render_text(), 3, false),
        vec!["abc"]
    );
    assert_eq!(stream.drain_renderable_markdown(3, false), None);
}

#[test]
fn markdown_drain_uses_code_block_content_width() {
    let mut stream = AppendOnlyStream::default();

    stream.push_delta("abcd ");
    let fragment = stream.drain_renderable_markdown(8, true).unwrap();
    assert_eq!(fragment.text.as_str(), "abcd");
    assert_eq!(
        rendered_markdown_text(fragment.render_text(), 8, true),
        vec!["│ abcd │"]
    );
    assert_eq!(stream.drain_renderable_markdown(8, true), None);
}

#[test]
fn markdown_drain_waits_when_second_raw_url_is_incomplete() {
    let mut stream = AppendOnlyStream::default();
    let text = "https://one.test https://two";

    stream.push_delta(text);
    assert_eq!(
        stream.drain_renderable_markdown(text.chars().count(), false),
        None
    );
    assert_eq!(stream.emitted_text(), "");

    stream.push_delta(".test ");
    let fragment = stream
        .drain_renderable_markdown(text.chars().count(), false)
        .unwrap();
    assert_eq!(fragment.text.as_str(), "https://one.test ");
    assert_eq!(stream.emitted_text(), "https://one.test ");
}

#[test]
fn markdown_drain_allows_complete_literal_brackets() {
    let mut stream = AppendOnlyStream::default();

    stream.push_delta("arr[0] ");
    let fragment = stream.drain_renderable_markdown(6, false).unwrap();
    assert_eq!(fragment.text.as_str(), "arr[0]");
    assert_eq!(stream.emitted_text(), "arr[0]");
}

#[test]
fn markdown_drain_allows_literal_unmatched_markers() {
    let mut stream = AppendOnlyStream::default();

    stream.push_delta("* item ");
    let fragment = stream.drain_renderable_markdown(6, false).unwrap();
    assert_eq!(fragment.text.as_str(), "* item");
    assert_eq!(stream.emitted_text(), "* item");
}

#[test]
fn markdown_drain_emits_complete_long_spans() {
    let mut stream = AppendOnlyStream::default();

    stream.push_delta("**supercalifragilistic** ");
    let fragment = stream.drain_renderable_markdown(5, false).unwrap();
    assert_eq!(fragment.text.as_str(), "**supercalifragilistic**");
    assert_eq!(stream.emitted_text(), "**supercalifragilistic**");
}

#[test]
fn markdown_drain_hard_wraps_code_block_content() {
    let mut stream = AppendOnlyStream::default();

    stream.push_delta("ab cd");
    let fragment = stream.drain_renderable_markdown(8, true).unwrap();
    assert_eq!(fragment.text.as_str(), "ab c");
    assert_eq!(stream.emitted_text(), "ab c");
}

#[test]
fn markdown_drain_uses_display_width_in_code_blocks() {
    let mut stream = AppendOnlyStream::default();

    stream.push_delta("你a");
    let fragment = stream.drain_renderable_markdown(6, true).unwrap();
    assert_eq!(fragment.text.as_str(), "你");
    assert_eq!(stream.emitted_text(), "你");
    assert_eq!(stream.drain_renderable_markdown(6, true), None);
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
