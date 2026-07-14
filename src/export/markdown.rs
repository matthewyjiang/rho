use std::{ops::Range, sync::OnceLock};

use katex::{KatexContext, OutputFormat, Settings};
use pulldown_cmark::{html, CowStr, Event, Options, Parser, Tag};

pub(super) fn to_html(text: &str) -> String {
    let prepared = prepare_math(text);
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
    let parser = Parser::new_ext(&prepared.source, options)
        .into_offset_iter()
        .map(|(event, range)| {
            if let Some(math) = prepared
                .math
                .iter()
                .find(|math| math.placeholder_range.start == range.start)
            {
                return math_event(context, &mut settings, &math.tex, math.display, math.syntax);
            }

            match event {
                Event::Html(raw) | Event::InlineHtml(raw) => Event::Text(raw),
                Event::InlineMath(tex) if is_ambiguous_inline_math(&prepared.source, range.end) => {
                    Event::Text(CowStr::Boxed(format!("${tex}$").into_boxed_str()))
                }
                Event::InlineMath(tex) => math_event(
                    context,
                    &mut settings,
                    &tex,
                    MathDisplay::Inline,
                    MathSyntax::Dollar,
                ),
                // Valid display math is extracted before Markdown parsing so
                // block markers inside it cannot become lists or headings.
                Event::DisplayMath(tex) => math_event(
                    context,
                    &mut settings,
                    &tex,
                    MathDisplay::Block,
                    MathSyntax::Dollar,
                ),
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

struct PreparedMarkdown {
    source: String,
    math: Vec<ExtractedMath>,
}

struct ExtractedMath {
    placeholder_range: Range<usize>,
    tex: String,
    display: MathDisplay,
    syntax: MathSyntax,
}

fn prepare_math(text: &str) -> PreparedMarkdown {
    let protected = protected_markdown_ranges(text);
    let mut protected_index = 0;
    let mut source = String::with_capacity(text.len());
    let mut math = Vec::new();
    let mut index = 0;

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
            source.push_str(&text[range.clone()]);
            index = range.end;
            protected_index += 1;
            continue;
        }

        if let Some(delimiter) = MathDelimiter::opening_at(text, index) {
            let content_start = index + delimiter.open().len();
            let protected_start = protected.get(protected_index).map(|range| range.start);
            if let Some(close_start) =
                find_closing_delimiter(text, content_start, delimiter.close(), protected_start)
            {
                let placeholder = format!("<rho-math-{} />", math.len());
                let placeholder_start = source.len();
                source.push_str(&placeholder);
                math.push(ExtractedMath {
                    placeholder_range: placeholder_start..source.len(),
                    tex: text[content_start..close_start].to_owned(),
                    display: delimiter.display(),
                    syntax: delimiter.syntax(),
                });
                index = close_start + delimiter.close().len();
                continue;
            }
        }

        let ch = text[index..]
            .chars()
            .next()
            .expect("index remains on a character boundary");
        source.push(ch);
        index += ch.len_utf8();
    }

    PreparedMarkdown { source, math }
}

fn find_closing_delimiter(
    text: &str,
    mut index: usize,
    delimiter: &str,
    protected_start: Option<usize>,
) -> Option<usize> {
    while index < text.len() && protected_start.is_none_or(|start| index < start) {
        if starts_with_unescaped(text, index, delimiter) {
            return Some(index);
        }
        let ch = text[index..]
            .chars()
            .next()
            .expect("index remains on a character boundary");
        index += ch.len_utf8();
    }
    None
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

#[derive(Clone, Copy)]
enum MathDelimiter {
    TexInline,
    TexBlock,
    DollarBlock,
}

impl MathDelimiter {
    fn opening_at(text: &str, index: usize) -> Option<Self> {
        [Self::TexBlock, Self::TexInline, Self::DollarBlock]
            .into_iter()
            .find(|delimiter| starts_with_unescaped(text, index, delimiter.open()))
    }

    fn open(self) -> &'static str {
        match self {
            Self::TexInline => r"\(",
            Self::TexBlock => r"\[",
            Self::DollarBlock => "$$",
        }
    }

    fn close(self) -> &'static str {
        match self {
            Self::TexInline => r"\)",
            Self::TexBlock => r"\]",
            Self::DollarBlock => "$$",
        }
    }

    fn display(self) -> MathDisplay {
        match self {
            Self::TexInline => MathDisplay::Inline,
            Self::TexBlock | Self::DollarBlock => MathDisplay::Block,
        }
    }

    fn syntax(self) -> MathSyntax {
        match self {
            Self::TexInline | Self::TexBlock => MathSyntax::Tex,
            Self::DollarBlock => MathSyntax::Dollar,
        }
    }
}

fn is_ambiguous_inline_math(text: &str, range_end: usize) -> bool {
    text[range_end..]
        .chars()
        .next()
        .is_some_and(char::is_alphanumeric)
}

#[derive(Clone, Copy)]
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
