use super::*;
use crate::tui::ReasoningChrome;

fn live_line_text(app: &mut App) -> Vec<String> {
    app.history_live_lines(80, crate::tui::DEFAULT_TUI_HEIGHT as usize)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

fn live_contains_thinking(app: &mut App) -> bool {
    live_line_text(app)
        .iter()
        .any(|line| line.contains("Thinking..."))
}

fn history_text(app: &mut App) -> Vec<String> {
    let area = ratatui::layout::Rect::new(0, 0, 80, crate::tui::DEFAULT_TUI_HEIGHT);
    let (settings, history_len, live) = {
        let ctx = app.frame_context(area);
        (ctx.settings, ctx.history_len, ctx.live_history.lines)
    };
    app.visible_history_lines_with_live(80, settings, 0, history_len, &live)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

// Covers: exclusive reasoning chrome matrix from zen + show_reasoning_output.
// Owner: RuntimeModelView display policy.
#[test]
fn reasoning_chrome_matrix() {
    let mut runtime = test_bootstrap().runtime;

    runtime.zen_mode = false;
    runtime.show_reasoning_output = true;
    assert_eq!(runtime.reasoning_chrome(), ReasoningChrome::FullText);
    assert!(runtime.displays_reasoning_output());
    assert!(runtime.shows_work_chrome());

    runtime.show_reasoning_output = false;
    assert_eq!(
        runtime.reasoning_chrome(),
        ReasoningChrome::ThinkingPlaceholder
    );
    assert!(!runtime.displays_reasoning_output());
    assert!(runtime.shows_work_chrome());

    runtime.zen_mode = true;
    runtime.show_reasoning_output = true;
    assert_eq!(runtime.reasoning_chrome(), ReasoningChrome::Hidden);
    assert!(!runtime.displays_reasoning_output());
    assert!(!runtime.shows_work_chrome());

    runtime.show_reasoning_output = false;
    assert_eq!(runtime.reasoning_chrome(), ReasoningChrome::Hidden);
}

// Covers: Thinking... is a render-time chrome decision, not phase state.
// Owner: history_live_lines + reasoning stretch lifecycle.
#[test]
fn thinking_placeholder_renders_only_for_open_thinking_chrome() {
    let mut app = test_app();
    app.info.runtime.zen_mode = false;
    app.info.runtime.show_reasoning_output = false;
    app.turn.reasoning_phase_mut().begin_step();

    assert!(live_contains_thinking(&mut app));

    // Full reasoning text: stretch can stay open; Thinking... must not paint.
    app.info.runtime.show_reasoning_output = true;
    assert_eq!(
        app.info.runtime.reasoning_chrome(),
        ReasoningChrome::FullText
    );
    assert!(app.turn.reasoning_phase().is_open());
    assert!(!live_contains_thinking(&mut app));

    // Zen hides Thinking... even while the stretch is open.
    app.info.runtime.zen_mode = true;
    app.info.runtime.show_reasoning_output = false;
    assert_eq!(app.info.runtime.reasoning_chrome(), ReasoningChrome::Hidden);
    assert!(!live_contains_thinking(&mut app));

    // Leaving zen with reasoning text off restores Thinking... while open.
    app.info.runtime.zen_mode = false;
    assert_eq!(
        app.info.runtime.reasoning_chrome(),
        ReasoningChrome::ThinkingPlaceholder
    );
    assert!(live_contains_thinking(&mut app));

    // Finalize closes the stretch; Thinking... ends without a policy change.
    assert!(app.turn.reasoning_phase_mut().finalize().is_none());
    assert!(!app.turn.reasoning_phase().is_open());
    assert!(!live_contains_thinking(&mut app));
}

// Covers: Thinking... uses the previous entry's trailing spacer, so it does not
// sit one row below the Thought for receipt that replaces it.
// Owner: history_live_lines spacing
#[test]
fn thinking_placeholder_aligns_with_thought_for_receipt() {
    let mut app = test_app();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    app.info.runtime.zen_mode = false;
    app.info.runtime.show_reasoning_output = false;
    app.insert_entry(&Entry::User("hello".into()));
    app.turn.reasoning_phase_mut().begin_step();
    app.handle_agent_event(
        ViewModelEvent::ReasoningDelta("hidden plan".into()),
        &mut terminal,
    )
    .unwrap();

    let live = live_line_text(&mut app);
    pretty_assertions::assert_eq!(live.len(), 1, "unexpected live chrome: {live:?}");
    assert!(
        live[0].contains("Thinking..."),
        "live chrome should start on Thinking..., not a spacer: {live:?}"
    );
    let thinking_row = history_text(&mut app)
        .iter()
        .position(|line| line.contains("Thinking..."))
        .expect("Thinking... while the stretch is open");

    app.finish_streams();
    let after = history_text(&mut app);
    assert!(
        after.iter().all(|line| !line.contains("Thinking...")),
        "Thinking... must yield to the thought receipt: {after:?}"
    );
    let thought_row = after
        .iter()
        .position(|line| line.contains("Thought for"))
        .expect("Thought for after the stretch closes");
    pretty_assertions::assert_eq!(thinking_row, thought_row);
}

// Covers: reasoning deltas keep the stretch open under zen without painting Thinking...
// Owner: event path + render policy.
#[test]
fn zen_reasoning_deltas_do_not_render_thinking_placeholder() {
    let mut app = test_app();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    app.info.runtime.zen_mode = true;
    app.info.runtime.show_reasoning_output = true;
    app.turn.reasoning_phase_mut().begin_step();

    app.handle_agent_event(
        ViewModelEvent::ReasoningDelta("secret plan".into()),
        &mut terminal,
    )
    .unwrap();

    assert!(app.turn.reasoning_phase().is_open());
    assert_eq!(app.info.runtime.reasoning_chrome(), ReasoningChrome::Hidden);
    assert!(!live_contains_thinking(&mut app));
    assert!(!app.info.runtime.displays_reasoning_output());
}
