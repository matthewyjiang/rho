//! Render a reconstructed PTY screen to a static SVG.
//!
//! Uses inline presentation attributes only: GitHub's README SVG sanitizer drops
//! `<style>` blocks.

use crate::screen::{CellColor, ScreenCell, ScreenModel};

/// Color scheme for the static proof-plate SVG.
///
/// Matrix captures use ANSI indexed colors. The scheme maps those indexes (and
/// default/frame chrome) so one screen can render for dark or light docs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SvgColorScheme {
    /// GitHub-dark terminal well used by the README and dark docs proof plate.
    #[default]
    Dark,
    /// GitHub Primer light terminal well for the light docs proof plate.
    ///
    /// ANSI slots match github-vscode-theme `light_default` terminal.* tokens.
    Light,
}

/// GitHub-dark terminal well used by the docs proof plate.
pub const DEFAULT_BG: Rgb = Rgb::new(0x0d, 0x11, 0x17);
/// Default body text on the docs proof plate.
pub const DEFAULT_FG: Rgb = Rgb::new(0xc9, 0xd1, 0xd9);
/// Outer plate behind the terminal well.
pub const FRAME_BG: Rgb = Rgb::new(0x09, 0x0c, 0x10);
/// Hairline around the terminal well.
pub const FRAME_STROKE: Rgb = Rgb::new(0x30, 0x36, 0x3d);

/// Primer `canvas.default` well for the light docs proof plate.
pub const LIGHT_DEFAULT_BG: Rgb = Rgb::new(0xff, 0xff, 0xff);
/// Primer `fg.default` body text on the light proof plate.
pub const LIGHT_DEFAULT_FG: Rgb = Rgb::new(0x1f, 0x23, 0x28);
/// Primer `canvas.subtle` outer plate behind the light terminal well.
pub const LIGHT_FRAME_BG: Rgb = Rgb::new(0xf6, 0xf8, 0xfa);
/// Primer `border.default` hairline around the light terminal well.
pub const LIGHT_FRAME_STROKE: Rgb = Rgb::new(0xd0, 0xd7, 0xde);

const FONT_STACK: &str = "DejaVu Sans Mono, Liberation Mono, Consolas, monospace";

/// sRGB triple.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub fn css(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

/// Layout knobs for the static proof-plate SVG.
#[derive(Clone, Debug)]
pub struct SvgOptions {
    pub title: String,
    pub description: String,
    pub cell_width: f64,
    pub cell_height: f64,
    pub font_size: f64,
    pub padding: f64,
    pub outer_radius: f64,
    pub inner_radius: f64,
    pub color_scheme: SvgColorScheme,
    pub default_fg: Rgb,
    pub default_bg: Rgb,
    pub frame_bg: Rgb,
    pub frame_stroke: Rgb,
}

impl Default for SvgOptions {
    fn default() -> Self {
        Self::for_scheme(SvgColorScheme::Dark)
    }
}

impl SvgOptions {
    /// Build options with chrome and ANSI mapping for `scheme`.
    pub fn for_scheme(scheme: SvgColorScheme) -> Self {
        let (default_fg, default_bg, frame_bg, frame_stroke) = match scheme {
            SvgColorScheme::Dark => (DEFAULT_FG, DEFAULT_BG, FRAME_BG, FRAME_STROKE),
            SvgColorScheme::Light => (
                LIGHT_DEFAULT_FG,
                LIGHT_DEFAULT_BG,
                LIGHT_FRAME_BG,
                LIGHT_FRAME_STROKE,
            ),
        };
        Self {
            title: "Rho interactive terminal UI".into(),
            description: "A Rho terminal session captured from the deterministic PTY harness."
                .into(),
            // ~0.6em mono advance keeps columns aligned without measuring glyphs.
            cell_width: 9.6,
            cell_height: 20.0,
            font_size: 16.0,
            padding: 24.0,
            // Square well matches the docs "hard ink border" proof plate and keeps
            // full-width cell backgrounds from fighting rounded corner clipping.
            outer_radius: 0.0,
            inner_radius: 0.0,
            color_scheme: scheme,
            default_fg,
            default_bg,
            frame_bg,
            frame_stroke,
        }
    }
}

/// Convert the visible screen into an SVG document.
pub fn render_screen_svg(screen: &ScreenModel, options: &SvgOptions) -> String {
    let cols = f64::from(screen.cols());
    let rows = f64::from(screen.rows());
    let content_w = cols * options.cell_width;
    let content_h = rows * options.cell_height;
    let width = content_w + options.padding * 2.0;
    let height = content_h + options.padding * 2.0;
    let origin_x = options.padding;
    let origin_y = options.padding;
    // Baseline sits near the lower third of the cell box.
    let baseline = options.cell_height * 0.78;

    let mut parts = Vec::with_capacity(64);
    parts.push(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width:.0}\" height=\"{height:.0}\" viewBox=\"0 0 {width:.1} {height:.1}\" role=\"img\" aria-labelledby=\"title desc\">"
    ));
    parts.push(format!(
        "  <title id=\"title\">{}</title>",
        escape_xml(&options.title)
    ));
    parts.push(format!(
        "  <desc id=\"desc\">{}</desc>",
        escape_xml(&options.description)
    ));
    parts.push(format!(
        "  <rect width=\"{width:.1}\" height=\"{height:.1}\" rx=\"{:.1}\" fill=\"{}\"/>",
        options.outer_radius,
        options.frame_bg.css()
    ));
    parts.push(format!(
        "  <rect x=\"{origin_x:.1}\" y=\"{origin_y:.1}\" width=\"{content_w:.1}\" height=\"{content_h:.1}\" rx=\"{:.1}\" fill=\"{}\" stroke=\"{}\"/>",
        options.inner_radius,
        options.default_bg.css(),
        options.frame_stroke.css()
    ));

    for row in 0..screen.rows() {
        let y = origin_y + f64::from(row) * options.cell_height;
        let mut col = 0u16;
        while col < screen.cols() {
            let Some(cell) = screen.cell(row, col) else {
                col = col.saturating_add(1);
                continue;
            };
            if cell.wide_continuation {
                col = col.saturating_add(1);
                continue;
            }

            let (fg, bg) = resolve_colors(&cell, options);
            let width_cells = if cell.wide { 2u16 } else { 1u16 };
            let mut run_text = String::new();
            let mut run_cols = 0u16;
            let mut cursor = col;
            let style_key = StyleKey::from_cell(&cell, fg, bg);

            while cursor < screen.cols() {
                let Some(next) = screen.cell(row, cursor) else {
                    break;
                };
                if next.wide_continuation {
                    break;
                }
                let (next_fg, next_bg) = resolve_colors(&next, options);
                if StyleKey::from_cell(&next, next_fg, next_bg) != style_key {
                    break;
                }
                let next_width = if next.wide { 2u16 } else { 1u16 };
                if cursor.saturating_add(next_width) > screen.cols() {
                    break;
                }
                let contents = if next.contents.is_empty() {
                    " "
                } else {
                    next.contents.as_str()
                };
                run_text.push_str(contents);
                run_cols = run_cols.saturating_add(next_width);
                cursor = cursor.saturating_add(next_width);
            }

            if run_cols == 0 {
                run_cols = width_cells;
                run_text = if cell.contents.is_empty() {
                    " ".into()
                } else {
                    cell.contents.clone()
                };
                cursor = col.saturating_add(run_cols);
            }

            let x = origin_x + f64::from(col) * options.cell_width;
            let run_w = f64::from(run_cols) * options.cell_width;

            if bg != options.default_bg {
                parts.push(format!(
                    "  <rect x=\"{x:.1}\" y=\"{y:.1}\" width=\"{run_w:.1}\" height=\"{:.1}\" fill=\"{}\"/>",
                    options.cell_height,
                    bg.css()
                ));
            }

            let trimmed_empty = run_text.chars().all(|ch| ch == ' ');
            if !trimmed_empty {
                let mut text_attrs = vec![
                    format!("x=\"{x:.1}\""),
                    format!("y=\"{:.1}\"", y + baseline),
                    format!("fill=\"{}\"", fg.css()),
                    format!("font-family=\"{FONT_STACK}\""),
                    format!("font-size=\"{:.1}\"", options.font_size),
                    "xml:space=\"preserve\"".into(),
                    "font-variant-ligatures=\"none\"".into(),
                ];
                if style_key.bold {
                    text_attrs.push("font-weight=\"700\"".into());
                }
                if style_key.italic {
                    text_attrs.push("font-style=\"italic\"".into());
                }
                if style_key.underline {
                    text_attrs.push("text-decoration=\"underline\"".into());
                }
                // Dim is applied via color resolution, not opacity, so GitHub keeps it.
                parts.push(format!(
                    "  <text {}>{}</text>",
                    text_attrs.join(" "),
                    escape_xml(&run_text)
                ));
            }

            col = cursor;
        }
    }

    parts.push("</svg>".into());
    parts.join("\n") + "\n"
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StyleKey {
    fg: Rgb,
    bg: Rgb,
    bold: bool,
    italic: bool,
    underline: bool,
}

impl StyleKey {
    fn from_cell(cell: &ScreenCell, fg: Rgb, bg: Rgb) -> Self {
        Self {
            fg,
            bg,
            bold: cell.bold && !cell.dim,
            italic: cell.italic,
            underline: cell.underline,
        }
    }
}

fn resolve_colors(cell: &ScreenCell, options: &SvgOptions) -> (Rgb, Rgb) {
    let mut fg = resolve_color(
        cell.fg,
        options.default_fg,
        /*is_fg*/ true,
        cell.bold,
        options.color_scheme,
    );
    let mut bg = resolve_color(
        cell.bg,
        options.default_bg,
        /*is_fg*/ false,
        cell.bold,
        options.color_scheme,
    );
    if cell.inverse {
        std::mem::swap(&mut fg, &mut bg);
    }
    if cell.dim {
        fg = dim_color(fg, bg);
    }
    (fg, bg)
}

fn resolve_color(
    color: CellColor,
    default: Rgb,
    is_fg: bool,
    bold: bool,
    scheme: SvgColorScheme,
) -> Rgb {
    match color {
        CellColor::Default => default,
        CellColor::Indexed(index) => indexed_color(index, is_fg, bold, scheme),
        // Truecolor cells keep the captured RGB. Matrix chrome is ANSI-indexed, so
        // light/dark plates diverge through the scheme map rather than RGB rewrite.
        CellColor::Rgb(r, g, b) => Rgb::new(r, g, b),
    }
}

fn dim_color(fg: Rgb, bg: Rgb) -> Rgb {
    // Blend toward the background so dim text stays readable on either well.
    Rgb::new(
        blend(fg.r, bg.r, 0.45),
        blend(fg.g, bg.g, 0.45),
        blend(fg.b, bg.b, 0.45),
    )
}

fn blend(from: u8, toward: u8, amount: f64) -> u8 {
    let value = f64::from(from) * (1.0 - amount) + f64::from(toward) * amount;
    value.round().clamp(0.0, 255.0) as u8
}

fn indexed_color(index: u8, is_fg: bool, bold: bool, scheme: SvgColorScheme) -> Rgb {
    // Brighten bold ANSI 0-7 foregrounds the way most terminals do.
    let index = if is_fg && bold && index < 8 {
        index + 8
    } else {
        index
    };
    match index {
        0..=15 => scheme.ansi16(index),
        16..=231 => {
            let value = index - 16;
            let r = value / 36;
            let g = (value % 36) / 6;
            let b = value % 6;
            Rgb::new(cube_component(r), cube_component(g), cube_component(b))
        }
        232..=255 => {
            let level = 8 + (index - 232) * 10;
            Rgb::new(level, level, level)
        }
    }
}

impl SvgColorScheme {
    /// Map ANSI 0-15 to the scheme palette.
    const fn ansi16(self, index: u8) -> Rgb {
        match self {
            // GitHub-dark terminal palette used by the original docs proof plate.
            Self::Dark => match index {
                0 => Rgb::new(0x0d, 0x11, 0x17),
                1 => Rgb::new(0xff, 0x7b, 0x72),
                2 => Rgb::new(0x3f, 0xb9, 0x50),
                3 => Rgb::new(0xd2, 0x99, 0x22),
                4 => Rgb::new(0x58, 0xa6, 0xff),
                5 => Rgb::new(0xbc, 0x8c, 0xff),
                6 => Rgb::new(0x39, 0xc5, 0xcf),
                7 => Rgb::new(0xc9, 0xd1, 0xd9),
                8 => Rgb::new(0x6e, 0x76, 0x81),
                9 => Rgb::new(0xff, 0xa1, 0x98),
                10 => Rgb::new(0x56, 0xd3, 0x64),
                11 => Rgb::new(0xe3, 0xb3, 0x41),
                12 => Rgb::new(0x79, 0xc0, 0xff),
                13 => Rgb::new(0xd2, 0xa8, 0xff),
                14 => Rgb::new(0x56, 0xd4, 0xdd),
                15 => Rgb::new(0xf0, 0xf3, 0xf6),
                _ => Rgb::new(0xc9, 0xd1, 0xd9),
            },
            // github-vscode-theme light_default terminal.ansi* / terminal.ansiBright*.
            // Index 15 stays near-white so block text on dark ANSI washes keeps contrast.
            // Body ink for Default cells comes from LIGHT_DEFAULT_FG, not this slot.
            Self::Light => match index {
                0 => Rgb::new(0x24, 0x29, 0x2f),
                1 => Rgb::new(0xcf, 0x22, 0x2e),
                2 => Rgb::new(0x11, 0x63, 0x29),
                3 => Rgb::new(0x4d, 0x2d, 0x00),
                4 => Rgb::new(0x09, 0x69, 0xda),
                5 => Rgb::new(0x82, 0x50, 0xdf),
                6 => Rgb::new(0x1b, 0x7c, 0x83),
                7 => Rgb::new(0x6e, 0x77, 0x81),
                8 => Rgb::new(0x57, 0x60, 0x6a),
                9 => Rgb::new(0xa4, 0x0e, 0x26),
                10 => Rgb::new(0x1a, 0x7f, 0x37),
                11 => Rgb::new(0x63, 0x3c, 0x01),
                12 => Rgb::new(0x21, 0x8b, 0xff),
                13 => Rgb::new(0xa4, 0x75, 0xf9),
                14 => Rgb::new(0x31, 0x92, 0xaa),
                15 => Rgb::new(0xff, 0xff, 0xff),
                _ => Rgb::new(0x1f, 0x23, 0x28),
            },
        }
    }
}

fn cube_component(value: u8) -> u8 {
    if value == 0 {
        0
    } else {
        55 + 40 * value
    }
}

fn escape_xml(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if c.is_control() && c != '\t' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    // Covers: XML specials in cell text must not break the SVG markup.
    // Owner: pty svg renderer
    #[test]
    fn escapes_xml_specials_in_text_runs() {
        let mut screen = ScreenModel::new(1, 20);
        screen.process(b"a<b>&\"c");
        let svg = render_screen_svg(&screen, &SvgOptions::default());
        assert!(svg.contains("a&lt;b&gt;&amp;&quot;c"));
        assert!(!svg.contains("a<b>"));
    }

    // Covers: inverse cells must swap resolved fg/bg in the static image.
    // Owner: pty svg renderer
    #[test]
    fn inverse_swaps_foreground_and_background() {
        let mut screen = ScreenModel::new(1, 8);
        screen.process(b"\x1b[7mABCD\x1b[m");
        let svg = render_screen_svg(&screen, &SvgOptions::default());
        assert!(svg.contains(&format!("fill=\"{}\"", DEFAULT_FG.css())));
        assert!(svg.contains("ABCD"));
    }

    // Covers: adjacent same-style cells collapse into one text run.
    // Owner: pty svg renderer
    #[test]
    fn groups_adjacent_same_style_cells() {
        let mut screen = ScreenModel::new(1, 16);
        screen.process(b"\x1b[32mhello\x1b[m");
        let svg = render_screen_svg(&screen, &SvgOptions::default());
        let occurrences = svg.matches(">hello</text>").count();
        assert_eq!(occurrences, 1);
    }

    // Covers: wide glyphs occupy two columns and must not double-render.
    // Owner: pty svg renderer
    #[test]
    fn wide_characters_render_once() {
        let mut screen = ScreenModel::new(1, 8);
        // Fullwidth LATIN CAPITAL LETTER A (U+FF21) is width 2.
        screen.process("\u{FF21}".as_bytes());
        let cell = screen.cell(0, 0).expect("wide cell");
        assert!(cell.wide);
        let cont = screen.cell(0, 1).expect("continuation cell");
        assert!(cont.wide_continuation);
        let svg = render_screen_svg(&screen, &SvgOptions::default());
        assert_eq!(svg.matches("\u{FF21}").count(), 1);
    }

    // Covers: light scheme must change plate chrome and semantic ANSI colors.
    // Owner: pty svg renderer
    #[test]
    fn light_scheme_remaps_chrome_and_ansi_colors() {
        let mut screen = ScreenModel::new(1, 16);
        screen.process(b"\x1b[32mok\x1b[m \x1b[90mdim\x1b[m");

        let dark = render_screen_svg(&screen, &SvgOptions::for_scheme(SvgColorScheme::Dark));
        let light = render_screen_svg(&screen, &SvgOptions::for_scheme(SvgColorScheme::Light));

        assert!(dark.contains(&format!("fill=\"{}\"", DEFAULT_BG.css())));
        assert!(light.contains(&format!("fill=\"{}\"", LIGHT_DEFAULT_BG.css())));
        assert!(dark.contains(&format!("fill=\"{}\"", FRAME_BG.css())));
        assert!(light.contains(&format!("fill=\"{}\"", LIGHT_FRAME_BG.css())));

        let dark_green = SvgColorScheme::Dark.ansi16(2).css();
        let light_green = SvgColorScheme::Light.ansi16(2).css();
        assert_ne!(dark_green, light_green);
        assert!(dark.contains(&format!("fill=\"{dark_green}\"")));
        assert!(light.contains(&format!("fill=\"{light_green}\"")));

        let dark_dim = SvgColorScheme::Dark.ansi16(8).css();
        let light_dim = SvgColorScheme::Light.ansi16(8).css();
        assert_ne!(dark_dim, light_dim);
        assert!(dark.contains(&format!("fill=\"{dark_dim}\"")));
        assert!(light.contains(&format!("fill=\"{light_dim}\"")));
    }

    // Covers: bright white on a dim wash stays light-colored on the light plate.
    // Owner: pty svg renderer
    #[test]
    fn light_scheme_keeps_bright_white_on_dim_wash() {
        let mut screen = ScreenModel::new(1, 12);
        // Dim wash (index 8) with bright white text (index 97 / 15).
        screen.process(b"\x1b[100;97mhello\x1b[m");

        let light = render_screen_svg(&screen, &SvgOptions::for_scheme(SvgColorScheme::Light));
        let wash = SvgColorScheme::Light.ansi16(8).css();
        let bright_white = SvgColorScheme::Light.ansi16(15).css();
        assert_eq!(bright_white, "#ffffff");
        assert!(light.contains(&format!("fill=\"{wash}\"")));
        assert!(light.contains(&format!("fill=\"{bright_white}\"")));
        assert!(light.contains(">hello</text>"));
    }

    // Covers: default SvgOptions stays on the dark GitHub well (README contract).
    // Owner: pty svg renderer
    #[test]
    fn default_options_use_dark_scheme() {
        let options = SvgOptions::default();
        assert_eq!(options.color_scheme, SvgColorScheme::Dark);
        assert_eq!(options.default_bg, DEFAULT_BG);
        assert_eq!(options.default_fg, DEFAULT_FG);
    }
}
