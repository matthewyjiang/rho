use std::sync::OnceLock;

use katex::{KatexContext, OutputFormat, Settings};
use pulldown_cmark::{html, CowStr, Event, Options, Parser};

pub(super) fn to_html(text: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_MATH);

    let context = math_context();
    let mut settings = Settings {
        output: OutputFormat::Mathml,
        ..Settings::default()
    };
    let parser =
        Parser::new_ext(text, options)
            .into_offset_iter()
            .map(|(event, range)| match event {
                Event::Html(raw) | Event::InlineHtml(raw) => Event::Text(raw),
                Event::InlineMath(tex) if is_ambiguous_inline_math(text, range.end) => {
                    Event::Text(CowStr::Boxed(format!("${tex}$").into_boxed_str()))
                }
                Event::InlineMath(tex) => {
                    math_event(context, &mut settings, &tex, MathDisplay::Inline)
                }
                Event::DisplayMath(tex) => {
                    math_event(context, &mut settings, &tex, MathDisplay::Block)
                }
                event => event,
            });
    let mut rendered = String::new();
    html::push_html(&mut rendered, parser);
    rendered
}

fn math_context() -> &'static KatexContext {
    static CONTEXT: OnceLock<KatexContext> = OnceLock::new();
    CONTEXT.get_or_init(KatexContext::default)
}

fn is_ambiguous_inline_math(text: &str, range_end: usize) -> bool {
    text[range_end..]
        .chars()
        .next()
        .is_some_and(char::is_alphanumeric)
}

#[derive(Clone, Copy)]
enum MathDisplay {
    Inline,
    Block,
}

fn math_event<'a>(
    context: &KatexContext,
    settings: &mut Settings,
    tex: &str,
    display: MathDisplay,
) -> Event<'a> {
    settings.display_mode = matches!(display, MathDisplay::Block);
    let markup = katex::render_to_string(context, tex, settings)
        .unwrap_or_else(|_| fallback_markup(tex, display));
    Event::InlineHtml(CowStr::Boxed(markup.into_boxed_str()))
}

fn fallback_markup(tex: &str, display: MathDisplay) -> String {
    let delimiter = match display {
        MathDisplay::Inline => "$",
        MathDisplay::Block => "$$",
    };
    format!(
        "<code class=\"math-fallback\">{delimiter}{}{delimiter}</code>",
        super::escape_html(tex)
    )
}
