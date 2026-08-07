use super::*;
use pretty_assertions::assert_eq;

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn rendered(source: &str, width: usize) -> Vec<String> {
    match render_math(source, width) {
        MathRender::Rendered(lines) => lines.iter().map(line_text).collect(),
        MathRender::Fallback(reason) => {
            panic!("unexpected fallback for {source:?}: {reason:?}")
        }
    }
}

// Covers: display-math delimiter scan must only claim closed $$ blocks
// Owner: pure unit (markdown math scan)
#[test]
fn scans_single_line_and_multi_line_display_math() {
    assert_eq!(
        display_math_span(&[r"$$x^2$$"]),
        Some(DisplayMathSpan::Complete { line_count: 1 })
    );
    assert_eq!(
        take_closed_display_math(&[r"$$x^2$$"]),
        Some(("x^2".into(), 1))
    );

    let multi = ["$$", r"\frac{a}{b}", "$$"];
    assert_eq!(
        display_math_span(&multi),
        Some(DisplayMathSpan::Complete { line_count: 3 })
    );
    assert_eq!(
        take_closed_display_math(&multi),
        Some((r"\frac{a}{b}".into(), 3))
    );

    assert_eq!(
        display_math_span(&["$$", r"x^2"]),
        Some(DisplayMathSpan::Incomplete)
    );
    assert_eq!(take_closed_display_math(&["$$", r"x^2"]), None);
    assert_eq!(display_math_span(&[r"$$partial"]), None);
    assert_eq!(display_math_span(&["not math"]), None);
    assert_eq!(display_math_span(&[r"$inline$"]), None);
}

// Covers: txm widget path must paint usable Unicode art for core latex
// Owner: pure unit (markdown math render)
#[test]
fn renders_core_latex_without_ansi_or_width_overflow() {
    for source in [
        r"E = mc^2",
        r"x^2 + y^2 = z^2",
        r"\frac{a}{b}",
        r"\sum_{n=1}^{N} n",
        r"\sqrt{x^2 + y^2}",
    ] {
        let lines = rendered(source, 80);
        assert!(!lines.is_empty(), "{source}");
        assert!(lines.iter().all(|line| !line.contains('\x1b')), "{source}");
        assert!(
            lines.iter().all(|line| display_width(line) <= 80),
            "{source}"
        );
    }

    let fraction = rendered(r"\frac{a}{b}", 80);
    assert!(
        fraction
            .iter()
            .any(|line| line.contains('─') || line.contains('-')),
        "{fraction:?}"
    );
    assert!(
        fraction.iter().any(|line| line.contains('a')),
        "{fraction:?}"
    );
    assert!(
        fraction.iter().any(|line| line.contains('b')),
        "{fraction:?}"
    );
}

// Covers: TXM blank padding rows must not inflate math panels
// Owner: pure unit (markdown math blank trim)
#[test]
fn drops_whitespace_only_rendered_rows() {
    let lines = rendered(
        r"\epsilon = \frac{1}{1 + \frac{1}{1 + \frac{1}{1 + \dots}}}",
        80,
    );
    assert!(
        lines
            .iter()
            .all(|line| line.chars().any(|ch| !ch.is_whitespace())),
        "expected no blank rows, got {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains('ε') || line.contains("epsilon")),
        "{lines:?}"
    );
    // Nested continued fraction should stay a compact stack, not a sparse padded block.
    assert!(
        lines.len() <= 8,
        "expected compact nested fraction, got {} rows: {lines:?}",
        lines.len()
    );
}

// Covers: inline math must accept only single-row art and reject taller output
// Owner: pure unit (markdown inline math render)
#[test]
fn inline_math_renders_single_row_only() {
    assert_eq!(render_inline_math("x^2"), Some("x²".into()));
    assert_eq!(render_inline_math("a_i"), Some("aᵢ".into()));

    let greek = render_inline_math(r"\alpha + \beta").expect("single-row greek");
    assert!(greek.contains('α') && greek.contains('β'), "{greek}");

    // Taller-than-one-row constructs keep their literal source.
    assert_eq!(render_inline_math(r"\frac{1}{2}"), None);
    assert_eq!(render_inline_math(r"\sum_{i=1}^{n} i"), None);
    assert_eq!(render_inline_math("x_i^2"), None);
}

// Covers: invalid or oversized inline sources must fall back without panicking
// Owner: pure unit (markdown inline math limits)
#[test]
fn inline_math_falls_back_for_blank_invalid_and_oversized_input() {
    assert_eq!(render_inline_math(""), None);
    assert_eq!(render_inline_math("   "), None);
    assert_eq!(render_inline_math("{"), None);
    assert_eq!(render_inline_math("a\nb"), None);
    assert_eq!(
        render_inline_math(&"x".repeat(MAX_INLINE_SOURCE_BYTES + 1)),
        None
    );
}

// Covers: bad or oversized math must fall back instead of panicking the TUI
// Owner: pure unit (markdown math limits)
#[test]
fn falls_back_for_blank_invalid_and_oversized_input() {
    assert_eq!(
        render_math("   \n", 80),
        MathRender::Fallback(MathFallback::Blank)
    );
    assert_eq!(
        render_math("{", 80),
        MathRender::Fallback(MathFallback::Parse)
    );
    assert_eq!(
        render_math(&"x".repeat(MAX_SOURCE_BYTES + 1), 80),
        MathRender::Fallback(MathFallback::SourceBytes)
    );
    let too_many_lines = std::iter::repeat_n("x", MAX_SOURCE_LINES + 1)
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        render_math(&too_many_lines, 80),
        MathRender::Fallback(MathFallback::SourceLines)
    );
    assert_eq!(
        render_math(r"\frac{a}{b}", 0),
        MathRender::Fallback(MathFallback::TooWide)
    );
    assert_eq!(
        render_math(r"\frac{a}{b}", 1),
        MathRender::Fallback(MathFallback::TooWide)
    );
}

// Covers: absolute rendered-width cap must not be reported as a narrow pane
// Owner: pure unit (markdown math output width limit)
#[test]
fn falls_back_for_output_exceeding_max_rendered_width() {
    // Long single-token source renders wider than MAX_RENDERED_WIDTH even in a
    // huge pane; that absolute cap must not look like a narrow-pane failure.
    let source = "a".repeat(MAX_RENDERED_WIDTH.saturating_add(32));
    assert_eq!(
        render_math(&source, MAX_RENDERED_WIDTH.saturating_mul(4)),
        MathRender::Fallback(MathFallback::OutputWidth)
    );
    assert_eq!(MathFallback::OutputWidth.panel_title(), "MATH · TOO WIDE");
    assert_eq!(
        MathFallback::TooWide.panel_title(),
        "MATH · PANE TOO NARROW"
    );
}

// Covers: closed fence helper must keep latex source for copy / fallback panels
// Owner: pure unit (markdown math fence)
#[test]
fn closed_fence_keeps_source_for_art_and_fallback() {
    match render_closed_display_math(r"\frac{a}{b}".into(), 80) {
        ClosedPanel::Art {
            title,
            lines,
            source,
        } => {
            assert_eq!(title, "MATH");
            assert_eq!(source, r"\frac{a}{b}");
            assert!(!lines.is_empty());
        }
        other => panic!("expected art, got {other:?}"),
    }

    match render_closed_display_math(String::new(), 80) {
        ClosedPanel::SourceFallback { title, source } => {
            assert_eq!(title, "MATH · NOT RENDERED");
            assert!(source.is_empty());
        }
        other => panic!("expected fallback, got {other:?}"),
    }
}
