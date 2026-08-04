use std::time::Instant;

use super::*;
use crate::tui::ReasoningChrome;

fn live_line_text(app: &App) -> Vec<String> {
    app.history_live_lines(80, Instant::now())
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

fn live_contains_thinking(app: &App) -> bool {
    live_line_text(app)
        .iter()
        .any(|line| line.contains("Thinking..."))
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

    assert!(live_contains_thinking(&app));

    // Full reasoning text: stretch can stay open; Thinking... must not paint.
    app.info.runtime.show_reasoning_output = true;
    assert_eq!(
        app.info.runtime.reasoning_chrome(),
        ReasoningChrome::FullText
    );
    assert!(app.turn.reasoning_phase().is_open());
    assert!(!live_contains_thinking(&app));

    // Zen hides Thinking... even while the stretch is open.
    app.info.runtime.zen_mode = true;
    app.info.runtime.show_reasoning_output = false;
    assert_eq!(app.info.runtime.reasoning_chrome(), ReasoningChrome::Hidden);
    assert!(!live_contains_thinking(&app));

    // Leaving zen with reasoning text off restores Thinking... while open.
    app.info.runtime.zen_mode = false;
    assert_eq!(
        app.info.runtime.reasoning_chrome(),
        ReasoningChrome::ThinkingPlaceholder
    );
    assert!(live_contains_thinking(&app));

    // Finalize closes the stretch; Thinking... ends without a policy change.
    assert!(app.turn.reasoning_phase_mut().finalize().is_none());
    assert!(!app.turn.reasoning_phase().is_open());
    assert!(!live_contains_thinking(&app));
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
    assert!(!live_contains_thinking(&app));
    assert!(!app.info.runtime.displays_reasoning_output());
}
