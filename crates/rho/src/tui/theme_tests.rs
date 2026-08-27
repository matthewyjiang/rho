use super::*;
use crate::tui::theme_terminal::{parse_palette_response, windows_console_palette};
use pretty_assertions::assert_eq;
use std::collections::HashMap;

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
    let _guard = theme_test_lock();
    Theme::apply_committed("terminal");
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
    let _guard = theme_test_lock();
    Theme::apply_committed("terminal");
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
    let _guard = theme_test_lock();
    Theme::apply_committed("terminal");
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

// Covers: fixed light schemes keep RGB dim/ink (no ANSI Black hole under Clear)
// Owner: tui theme palette mapping
#[test]
fn fixed_light_scheme_uses_rgb_dim_and_surface() {
    let _guard = theme_test_lock();
    Theme::apply_committed("terminal");
    Theme::apply_committed("monochrome-light");
    let palette = Palette::current();
    assert!(matches!(palette.surface, Some(Color::Rgb(_, _, _))));
    assert!(matches!(palette.text, Some(Color::Rgb(_, _, _))));
    assert!(
        matches!(palette.dim, Color::Rgb(_, _, _)),
        "dim must be scheme RGB, got {:?}",
        palette.dim
    );
    let surface = Theme::surface();
    assert!(matches!(surface.bg, Some(Color::Rgb(_, _, _))));
    assert!(matches!(surface.fg, Some(Color::Rgb(_, _, _))));

    // Soft panels wash toward black on light surfaces, not white-on-white.
    let panel = palette.user_background.rgb.expect("panel rgb");
    let scheme = theme_scheme::resolve_fixed_scheme("monochrome-light").unwrap();
    assert!(
        panel.luminance() < scheme.background.luminance(),
        "panel wash should darken light surfaces"
    );

    Theme::apply_committed("terminal");
}

// Covers: rail dim must not collapse into the wash on terminal fallback or tight palettes
// Owner: tui theme palette mapping
#[test]
fn activity_rail_dim_stays_readable_on_terminal_wash() {
    let _guard = theme_test_lock();
    Theme::apply_committed("terminal");
    let rail = Theme::activity_rail();
    let dim = Theme::activity_rail_dim();
    assert_eq!(rail.bg, Some(Color::DarkGray));
    assert_eq!(dim.bg, rail.bg);
    assert_ne!(dim.fg, dim.bg, "dim rail text must not match the wash");
    assert_eq!(
        dim.fg,
        Some(Palette::current().panel_dim),
        "rail dim uses the palette's wash-safe muted ink"
    );
    assert_eq!(
        dim.fg,
        Some(Color::Gray),
        "named DarkGray wash keeps a muted slot, not body white"
    );

    Theme::apply_committed("one-half-dark");
    let rail = Theme::activity_rail();
    let dim = Theme::activity_rail_dim();
    let palette = Palette::current();
    assert_eq!(dim.bg, rail.bg);
    assert_ne!(dim.fg, dim.bg);
    assert_eq!(dim.fg, Some(palette.panel_dim));
    assert_eq!(
        palette.panel_dim, palette.dim,
        "built-in schemes keep muted rail ink when it still contrasts"
    );

    Theme::apply_committed("terminal");
}

// Covers: panel dim keeps usable muted ink and blends instead of jumping to body
// Owner: tui theme palette mapping
#[test]
fn panel_dim_keeps_muted_ink_instead_of_body_fallback() {
    let dark_bg = Rgb::new(10, 10, 10);
    let light_bg = Rgb::new(245, 245, 245);
    let white = Rgb::new(255, 255, 255);
    let black = Rgb::new(0, 0, 0);

    let usable = Palette::from_terminal(Some(&palette(dark_bg, Some(Rgb::new(170, 170, 170)))));
    assert_eq!(usable.dim, Color::Rgb(170, 170, 170));
    assert_eq!(usable.panel_dim, Color::Rgb(170, 170, 170));
    assert_ne!(usable.panel_dim, usable.neutral_tool_background.color);

    let unnamed = Palette::from_terminal(None);
    assert_eq!(unnamed.dim, Color::DarkGray);
    assert_eq!(unnamed.neutral_tool_background.color, Color::DarkGray);
    assert_eq!(unnamed.panel_dim, Color::Gray);

    let tight = dim_on_background(Rgb::new(20, 20, 20), [Rgb::new(30, 30, 30)], white);
    assert_eq!(tight, Color::Rgb(149, 149, 149));

    let light_tight = dim_on_background(Rgb::new(240, 240, 240), [Rgb::new(220, 220, 220)], black);
    assert_eq!(light_tight, Color::Rgb(108, 108, 108));

    let light_ok = dim_on_background(Rgb::new(240, 240, 240), [Rgb::new(80, 80, 80)], black);
    assert_eq!(light_ok, Color::Rgb(80, 80, 80));

    let no_sample = Palette::from_terminal(Some(&palette(dark_bg, None)));
    assert_eq!(no_sample.dim, Color::DarkGray);
    assert!(matches!(no_sample.panel_dim, Color::Rgb(_, _, _)));
    assert_ne!(no_sample.panel_dim, no_sample.neutral_tool_background.color);

    let light = Palette::from_terminal(Some(&palette(light_bg, Some(Rgb::new(80, 80, 80)))));
    assert_eq!(light.dim, Color::Rgb(80, 80, 80));
    assert_eq!(light.panel_dim, Color::Rgb(80, 80, 80));
}

// Covers: hover lift direction tracks surface luminance; non-RGB ink lifts to bold
// Owner: tui theme palette mapping
#[test]
fn hover_lift_blends_rgb_ink_toward_surface_opposite() {
    let _guard = theme_test_lock();
    Theme::apply_committed("terminal");
    Theme::apply_committed("one-half-dark");
    // Dark surface: mid gray lifts toward white, staying in band.
    let lifted = Theme::hover_lifted(Color::Rgb(140, 140, 140)).expect("rgb ink lifts");
    let Color::Rgb(red, green, blue) = lifted else {
        panic!("lifted ink must stay RGB, got {lifted:?}");
    };
    assert!(
        red > 140 && green > 140 && blue > 140,
        "dark-surface lift must brighten, got ({red},{green},{blue})"
    );

    Theme::apply_committed("monochrome-light");
    // Light surface: dark ink lifts toward black.
    let lifted = Theme::hover_lifted(Color::Rgb(100, 100, 100)).expect("rgb ink lifts");
    let Color::Rgb(red, green, blue) = lifted else {
        panic!("lifted ink must stay RGB, got {lifted:?}");
    };
    assert!(
        red < 100 && green < 100 && blue < 100,
        "light-surface lift must darken, got ({red},{green},{blue})"
    );

    // Named and default ink cannot blend; callers fall back to bold.
    for ink in [Color::Reset, Color::DarkGray, Color::White] {
        assert_eq!(
            Theme::hover_lifted(ink),
            None,
            "{ink:?} must fall back to bold"
        );
    }

    Theme::apply_committed("terminal");
}

// Covers: add/remove chrome uses role fg signs, soft wash, base content text
// Owner: tui theme palette mapping
#[test]
fn scheme_diff_chrome_washes_row_with_fg_signs() {
    let _guard = theme_test_lock();
    Theme::apply_committed("terminal");
    Theme::apply_committed("one-half-dark");
    let palette = Palette::current();
    let add_wash = palette.diff_add_wash.expect("add wash").color;
    let del_wash = palette.diff_del_wash.expect("del wash").color;
    assert_ne!(add_wash, del_wash);

    let add = Theme::tool_diff_chrome(rho_tools::tool_card::DiffRowKind::Added);
    let del = Theme::tool_diff_chrome(rho_tools::tool_card::DiffRowKind::Removed);
    // Signs: role foreground on the same soft wash as the row (not a solid gutter).
    assert_eq!(add.sign.fg, Some(palette.success));
    assert_eq!(del.sign.fg, Some(palette.error));
    assert_eq!(add.sign.bg, Some(add_wash));
    assert_eq!(del.sign.bg, Some(del_wash));
    // washed() carries the row wash onto column bases (content uses Theme::text).
    assert_eq!(add.washed(Theme::tool_diff_gutter()).bg, Some(add_wash));
    assert_eq!(del.washed(Theme::text()).bg, Some(del_wash));
    // Context has no wash or role sign.
    let ctx = Theme::tool_diff_chrome(rho_tools::tool_card::DiffRowKind::Context);
    assert_eq!(ctx.sign, Theme::text());
    assert_eq!(ctx.washed(Theme::text()).bg, None);

    Theme::apply_committed("terminal");
}

// Covers: brand/version roles use sampled terminal RGB, not bare Color::Cyan/Green
// Owner: tui theme palette mapping
#[test]
fn terminal_palette_drives_brand_and_success_rgb() {
    let ansi = HashMap::from([
        (AnsiColor::Cyan, Rgb::new(10, 20, 30)),
        (AnsiColor::Green, Rgb::new(40, 50, 60)),
        (AnsiColor::Yellow, Rgb::new(70, 80, 90)),
        (AnsiColor::Red, Rgb::new(100, 110, 120)),
        (AnsiColor::Magenta, Rgb::new(130, 140, 150)),
        (AnsiColor::White, Rgb::new(200, 200, 200)),
    ]);
    let terminal = TerminalPalette {
        background: Rgb::new(0, 0, 0),
        ansi,
    };
    let palette = Palette::from_terminal(Some(&terminal));
    assert_eq!(palette.accent, Color::Rgb(10, 20, 30));
    assert_eq!(palette.success, Color::Rgb(40, 50, 60));
    assert_eq!(palette.warning, Color::Rgb(70, 80, 90));
    assert_eq!(palette.error, Color::Rgb(100, 110, 120));
    assert_eq!(palette.skill, Color::Rgb(130, 140, 150));

    // Sampled green/red also drive optional diff row washes.
    assert!(palette.diff_add_wash.is_some());
    assert!(palette.diff_del_wash.is_some());

    // Without a sample, keep named ANSI so the host can still paint them.
    // Skip harsh named-ANSI backgrounds for diff chrome.
    let fallback = Palette::from_terminal(None);
    assert_eq!(fallback.accent, Color::Cyan);
    assert_eq!(fallback.success, Color::Green);
    assert!(fallback.diff_add_wash.is_none());
    assert!(fallback.diff_del_wash.is_none());
}

// Covers: bright warning yellow is darkened on light surfaces for Auto/status text
// Owner: tui theme palette mapping
#[test]
fn role_ink_darkens_bright_yellow_on_light_surface() {
    let light = Rgb::new(250, 250, 250);
    let bright_yellow = Color::Rgb(0xf1, 0xfa, 0x8c);
    let adjusted = role_ink(bright_yellow, Some(light));
    let Color::Rgb(red, green, blue) = adjusted else {
        panic!("expected rgb ink, got {adjusted:?}");
    };
    let ink = Rgb::new(red, green, blue);
    assert!(
        ink.luminance() + ROLE_INK_MARGIN <= light.luminance(),
        "warning ink should sit clearly below light surface: {ink:?}"
    );
    // Still warm, not pure gray/black.
    assert!(
        red > blue && green > blue,
        "should keep a warm yellow/amber cast"
    );

    let dark = Rgb::new(20, 20, 20);
    assert_eq!(
        role_ink(bright_yellow, Some(dark)),
        bright_yellow,
        "bright yellow stays on dark surfaces"
    );
}

// Covers: preview does not commit; cancel restores committed
// Owner: tui theme state
#[test]
fn preview_then_cancel_restores_committed() {
    let _guard = theme_test_lock();
    Theme::apply_committed("terminal");
    Theme::apply_committed("one-half-dark");
    assert_eq!(Theme::committed_id(), "one-half-dark");
    assert_eq!(Theme::active_id(), "one-half-dark");

    Theme::preview("monochrome-light");
    assert_eq!(Theme::committed_id(), "one-half-dark");
    assert_eq!(Theme::active_id(), "monochrome-light");

    Theme::cancel_preview();
    assert_eq!(Theme::committed_id(), "one-half-dark");
    assert_eq!(Theme::active_id(), "one-half-dark");

    Theme::apply_committed("terminal");
}

// Covers: apply_committed updates both active and committed
// Owner: tui theme state
#[test]
fn apply_committed_updates_active_and_committed() {
    let _guard = theme_test_lock();
    Theme::apply_committed("terminal");
    Theme::apply_committed("terminal");
    let gen = Theme::generation();
    Theme::apply_committed("one-half-light");
    assert_eq!(Theme::committed_id(), "one-half-light");
    assert_eq!(Theme::active_id(), "one-half-light");
    assert!(Theme::generation() > gen);
    Theme::apply_committed("terminal");
}
