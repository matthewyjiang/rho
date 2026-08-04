use super::*;
use pretty_assertions::assert_eq;

fn palette(background: Rgb, bright_black: Option<Rgb>) -> TerminalPalette {
    let mut ansi = HashMap::from([(AnsiColor::White, Rgb::new(255, 255, 255))]);
    if let Some(bright_black) = bright_black {
        ansi.insert(AnsiColor::BrightBlack, bright_black);
    }
    TerminalPalette { background, ansi }
}

fn dim_of(background: Rgb, bright_black: Option<Rgb>) -> Color {
    Palette::from_terminal(Some(&palette(background, bright_black))).dim
}

// Covers: dim resolves from bright black with fallbacks; never from ANSI white
// Owner: tui theme palette mapping
#[test]
fn dim_foreground_policy_table() {
    let dark_bg = Rgb::new(10, 10, 10);
    let light_bg = Rgb::new(245, 245, 245);

    let cases = [
        // usable bright black on dark bg
        (
            dark_bg,
            Some(Rgb::new(170, 170, 170)),
            Color::Rgb(170, 170, 170),
        ),
        // near-white bright black rejected on dark bg
        (dark_bg, Some(Rgb::new(245, 245, 245)), Color::DarkGray),
        // near-black bright black rejected on dark bg
        (dark_bg, Some(Rgb::new(8, 8, 8)), Color::DarkGray),
        // missing bright black falls back on dark bg
        (dark_bg, None, Color::DarkGray),
        // usable bright black on light bg
        (light_bg, Some(Rgb::new(80, 80, 80)), Color::Rgb(80, 80, 80)),
        // too-light bright black rejected on light bg
        (light_bg, Some(Rgb::new(200, 200, 200)), Color::Black),
        // missing bright black falls back on light bg
        (light_bg, None, Color::Black),
    ];

    for (background, bright_black, expected) in cases {
        assert_eq!(
            dim_of(background, bright_black),
            expected,
            "bg={background:?} bright_black={bright_black:?}"
        );
    }
}

// Covers: OSC parse keeps white required and bright black optional
// Owner: tui theme palette mapping
#[test]
fn parses_osc_palette_with_optional_bright_black() {
    let chromatic_and_white = "\x1b]11;rgb:0000/0000/0000\x1b\\\
\x1b]4;1;rgb:ffff/0000/0000\x1b\\\
\x1b]4;2;rgb:0000/ffff/0000\x1b\\\
\x1b]4;3;rgb:ffff/ffff/0000\x1b\\\
\x1b]4;4;rgb:0000/0000/ffff\x1b\\\
\x1b]4;5;rgb:ffff/0000/ffff\x1b\\\
\x1b]4;6;rgb:0000/ffff/ffff\x1b\\\
\x1b]4;7;rgb:ffff/ffff/ffff\x1b\\";

    let without_bright_black =
        parse_palette_response(chromatic_and_white).expect("palette without index 8");
    assert!(!without_bright_black
        .ansi
        .contains_key(&AnsiColor::BrightBlack));
    assert_eq!(
        Palette::from_terminal(Some(&without_bright_black)).dim,
        Color::DarkGray
    );

    let with_bright_black = format!("{chromatic_and_white}\x1b]4;8;rgb:aaaa/aaaa/aaaa\x1b\\");
    let palette = parse_palette_response(&with_bright_black).expect("palette with index 8");
    assert_eq!(palette.background, Rgb::new(0, 0, 0));
    assert_eq!(palette.ansi[&AnsiColor::Red], Rgb::new(255, 0, 0));
    assert_eq!(palette.ansi[&AnsiColor::White], Rgb::new(255, 255, 255));
    assert_eq!(
        palette.ansi[&AnsiColor::BrightBlack],
        Rgb::new(170, 170, 170)
    );
    assert_eq!(
        Palette::from_terminal(Some(&palette)).dim,
        Color::Rgb(170, 170, 170)
    );
}

#[test]
fn resolves_windows_console_palette_in_attribute_bit_order() {
    let color_table = [
        0x000000, 0x110000, 0x001100, 0x111100, 0x000011, 0x110011, 0x001111, 0xeeeeee, 0x222222,
        0x330000, 0x003300, 0x333300, 0x000033, 0x330033, 0x003333, 0x333333,
    ];

    let palette = windows_console_palette(&color_table, 0x20);

    assert_eq!(palette.background, Rgb::new(0, 17, 0));
    assert_eq!(palette.ansi[&AnsiColor::Red], Rgb::new(17, 0, 0));
    assert_eq!(palette.ansi[&AnsiColor::Blue], Rgb::new(0, 0, 17));
    assert_eq!(palette.ansi[&AnsiColor::White], Rgb::new(238, 238, 238));
    assert_eq!(palette.ansi[&AnsiColor::BrightBlack], Rgb::new(34, 34, 34));
}

#[test]
fn chooses_dark_block_foreground_for_light_resolved_backgrounds() {
    assert_eq!(
        block_foreground(Some(Rgb::new(240, 240, 240))),
        Color::Black
    );
    assert_eq!(block_foreground(Some(Rgb::new(20, 20, 20))), Color::White);
    assert_eq!(block_foreground(None), Color::White);
}

#[test]
fn blends_toward_terminal_ansi_color() {
    let base = Rgb::new(10, 10, 10);
    let green = Rgb::new(10, 110, 10);

    assert_eq!(base.blend_toward(green, 0.16), Rgb::new(10, 26, 10));
}

#[test]
fn resolved_ansi_background_keeps_rgb_for_foreground_contrast() {
    let palette = TerminalPalette {
        background: Rgb::new(255, 255, 255),
        ansi: HashMap::from([(AnsiColor::White, Rgb::new(240, 240, 240))]),
    };

    let background = palette
        .blended_background(AnsiColor::White, USER_BACKGROUND_ALPHA)
        .expect("resolved background");

    assert_eq!(background.color, Color::Rgb(254, 254, 254));
    assert_eq!(block_foreground(background.rgb), Color::Black);
}

// Covers: reasoning uses Theme::dim only, not a second DIM modifier
// Owner: tui theme palette mapping
#[test]
fn reasoning_output_uses_one_dimming_mechanism() {
    let styled = Style::default().add_modifier(Modifier::DIM | Modifier::ITALIC);
    let mut lines = vec![Line::styled("reasoning", styled).style(styled)];

    Theme::reasoning_output(&mut lines);

    for style in [lines[0].style, lines[0].spans[0].style] {
        let effective_modifiers = style.add_modifier - style.sub_modifier;
        assert!(!effective_modifiers.contains(Modifier::DIM));
        // No terminal palette in unit tests, so dim falls back to DarkGray.
        assert_eq!(style.fg, Some(Color::DarkGray));
    }
}
