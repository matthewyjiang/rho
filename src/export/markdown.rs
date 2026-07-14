use std::{ops::Range, sync::OnceLock};

use katex::{KatexContext, OutputFormat, Settings};
use pulldown_cmark::{html, CowStr, Event, Options, Parser, Tag};

pub(super) fn to_html(text: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_MATH);

    let normalized = normalize_tex_delimiters(text);
    let context = math_context();
    let mut settings = Settings {
        output: OutputFormat::Mathml,
        ..Settings::default()
    };
    let parser = Parser::new_ext(&normalized.source, options)
        .into_offset_iter()
        .map(|(event, range)| {
            let syntax = if normalized.is_explicit_math(&range) {
                MathSyntax::Tex
            } else {
                MathSyntax::Dollar
            };
            match event {
                Event::Html(raw) | Event::InlineHtml(raw) => Event::Text(raw),
                Event::InlineMath(tex)
                    if syntax == MathSyntax::Dollar
                        && is_ambiguous_inline_math(&normalized.source, range.end) =>
                {
                    Event::Text(CowStr::Boxed(format!("${tex}$").into_boxed_str()))
                }
                Event::InlineMath(tex) => {
                    math_event(context, &mut settings, &tex, MathDisplay::Inline, syntax)
                }
                Event::DisplayMath(tex) => {
                    math_event(context, &mut settings, &tex, MathDisplay::Block, syntax)
                }
                event => event,
            }
        });
    let mut rendered = String::new();
    html::push_html(&mut rendered, parser);
    rendered
}

fn math_context() -> &'static KatexContext {
    static CONTEXT: OnceLock<KatexContext> = OnceLock::new();
    CONTEXT.get_or_init(KatexContext::default)
}

struct NormalizedMarkdown {
    source: String,
    explicit_math: Vec<Range<usize>>,
}

impl NormalizedMarkdown {
    fn is_explicit_math(&self, range: &Range<usize>) -> bool {
        self.explicit_math.iter().any(|explicit| explicit == range)
    }
}

fn normalize_tex_delimiters(text: &str) -> NormalizedMarkdown {
    let protected = protected_markdown_ranges(text);
    let mut protected_index = 0;
    let mut output = String::with_capacity(text.len());
    let mut explicit_math = Vec::new();
    let mut index = 0;
    let mut open: Option<(TexDelimiter, usize)> = None;

    while index < text.len() {
        while protected
            .get(protected_index)
            .is_some_and(|range| range.end <= index)
        {
            protected_index += 1;
        }
        if let Some(range) = protected
            .get(protected_index)
            .filter(|range| range.start == index)
        {
            if let Some((delimiter, output_start)) = open.take() {
                output.replace_range(
                    output_start..output_start + delimiter.markdown().len(),
                    delimiter.open(),
                );
            }
            output.push_str(&text[range.clone()]);
            index = range.end;
            protected_index += 1;
            continue;
        }

        if let Some((delimiter, _)) = open {
            if starts_with_unescaped(text, index, delimiter.close()) {
                output.push_str(delimiter.markdown());
                index += delimiter.close().len();
                explicit_math.push(output_start(open, delimiter)..output.len());
                open = None;
                continue;
            }
        } else if let Some(delimiter) = TexDelimiter::opening_at(text, index) {
            let output_start = output.len();
            output.push_str(delimiter.markdown());
            index += delimiter.open().len();
            open = Some((delimiter, output_start));
            continue;
        }

        let ch = text[index..]
            .chars()
            .next()
            .expect("index remains on a character boundary");
        output.push(ch);
        index += ch.len_utf8();
    }

    if let Some((delimiter, output_start)) = open {
        output.replace_range(
            output_start..output_start + delimiter.markdown().len(),
            delimiter.open(),
        );
    }
    NormalizedMarkdown {
        source: output,
        explicit_math,
    }
}

fn output_start(open: Option<(TexDelimiter, usize)>, delimiter: TexDelimiter) -> usize {
    open.filter(|(open_delimiter, _)| *open_delimiter == delimiter)
        .map(|(_, output_start)| output_start)
        .expect("closing delimiter matches the open delimiter")
}

fn protected_markdown_ranges(text: &str) -> Vec<Range<usize>> {
    Parser::new(text)
        .into_offset_iter()
        .filter_map(|(event, range)| match event {
            Event::Code(_)
            | Event::Html(_)
            | Event::InlineHtml(_)
            | Event::Start(Tag::CodeBlock(_)) => Some(range),
            _ => None,
        })
        .collect()
}

fn starts_with_unescaped(text: &str, index: usize, delimiter: &str) -> bool {
    text[index..].starts_with(delimiter)
        && text[..index]
            .chars()
            .rev()
            .take_while(|ch| *ch == '\\')
            .count()
            .is_multiple_of(2)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TexDelimiter {
    Inline,
    Block,
}

impl TexDelimiter {
    fn opening_at(text: &str, index: usize) -> Option<Self> {
        if starts_with_unescaped(text, index, Self::Block.open()) {
            Some(Self::Block)
        } else if starts_with_unescaped(text, index, Self::Inline.open()) {
            Some(Self::Inline)
        } else {
            None
        }
    }

    fn open(self) -> &'static str {
        match self {
            Self::Inline => r"\(",
            Self::Block => r"\[",
        }
    }

    fn close(self) -> &'static str {
        match self {
            Self::Inline => r"\)",
            Self::Block => r"\]",
        }
    }

    fn markdown(self) -> &'static str {
        match self {
            Self::Inline => "$",
            Self::Block => "$$",
        }
    }
}

fn is_ambiguous_inline_math(text: &str, range_end: usize) -> bool {
    text[range_end..]
        .chars()
        .next()
        .is_some_and(char::is_alphanumeric)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MathSyntax {
    Dollar,
    Tex,
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
    syntax: MathSyntax,
) -> Event<'a> {
    settings.display_mode = matches!(display, MathDisplay::Block);
    let markup = katex::render_to_string(context, tex, settings)
        .unwrap_or_else(|_| fallback_markup(tex, display, syntax));
    Event::InlineHtml(CowStr::Boxed(markup.into_boxed_str()))
}

fn fallback_markup(tex: &str, display: MathDisplay, syntax: MathSyntax) -> String {
    let (open, close) = match (display, syntax) {
        (MathDisplay::Inline, MathSyntax::Dollar) => ("$", "$"),
        (MathDisplay::Block, MathSyntax::Dollar) => ("$$", "$$"),
        (MathDisplay::Inline, MathSyntax::Tex) => (r"\(", r"\)"),
        (MathDisplay::Block, MathSyntax::Tex) => (r"\[", r"\]"),
    };
    format!(
        "<code class=\"math-fallback\">{open}{}{close}</code>",
        super::escape_html(tex)
    )
}
