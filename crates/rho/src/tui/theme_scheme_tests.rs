use super::*;
use pretty_assertions::assert_eq;
use std::fs;

// Covers: hex parse accepts #RGB and #RRGGBB
// Owner: tui theme scheme parse
#[test]
fn parses_hex_colors() {
    assert_eq!(Rgb::from_hex("#112233"), Some(Rgb::new(0x11, 0x22, 0x33)));
    assert_eq!(Rgb::from_hex("aabbcc"), Some(Rgb::new(0xaa, 0xbb, 0xcc)));
    assert_eq!(Rgb::from_hex("#abc"), Some(Rgb::new(0xaa, 0xbb, 0xcc)));
    assert_eq!(Rgb::from_hex("zz"), None);
}

// Covers: Windows Terminal JSON maps into a full 16-color scheme
// Owner: tui theme scheme parse
#[test]
fn parses_windows_terminal_scheme_json() {
    let json = r##"{
      "name": "Example",
      "background": "#000000",
      "foreground": "#ffffff",
      "cursorColor": "#eeeeee",
      "selectionBackground": "#333333",
      "black": "#010101",
      "red": "#ff0000",
      "green": "#00ff00",
      "yellow": "#ffff00",
      "blue": "#0000ff",
      "purple": "#ff00ff",
      "cyan": "#00ffff",
      "white": "#f0f0f0",
      "brightBlack": "#808080",
      "brightRed": "#ff8080",
      "brightGreen": "#80ff80",
      "brightYellow": "#ffff80",
      "brightBlue": "#8080ff",
      "brightPurple": "#ff80ff",
      "brightCyan": "#80ffff",
      "brightWhite": "#ffffff"
    }"##;
    let file: WindowsTerminalScheme = serde_json::from_str(json).unwrap();
    let scheme = file
        .into_scheme("example", ThemeSourceKind::Custom)
        .unwrap();
    assert_eq!(scheme.name, "Example");
    assert_eq!(scheme.background, Rgb::new(0, 0, 0));
    assert_eq!(scheme.ansi[1], Rgb::new(255, 0, 0));
    assert_eq!(scheme.ansi[8], Rgb::new(128, 128, 128));
}

// Covers: built-in ids resolve; terminal id stays unfixed
// Owner: tui theme scheme catalog
#[test]
fn resolves_builtin_and_terminal_ids() {
    assert!(resolve_fixed_scheme(TERMINAL_THEME_ID).is_none());
    assert!(resolve_fixed_scheme("").is_none());
    let classic = resolve_fixed_scheme("one-half-dark").expect("one-half-dark");
    assert_eq!(classic.id, "one-half-dark");
    assert_eq!(classic.source, ThemeSourceKind::Builtin);
    assert!(resolve_fixed_scheme("missing-theme-xyz").is_none());
    assert_eq!(theme_display_name("one-half-dark"), "One Half Dark");
    assert_eq!(theme_display_name("terminal"), "Terminal");
    assert_eq!(theme_display_name(""), "Terminal");
}

// Covers: custom themes load from RHO_HOME/themes and skip reserved ids
// Owner: tui theme scheme catalog
#[test]
fn lists_custom_themes_from_rho_home() {
    let _guard = crate::paths::process_env_lock();
    let root = tempfile::tempdir().unwrap();
    let themes = root.path().join("themes");
    fs::create_dir_all(&themes).unwrap();
    fs::write(
        themes.join("forest.json"),
        r##"{
          "name": "Forest",
          "background": "#0b1a0b",
          "foreground": "#e8f5e8",
          "black": "#0b1a0b",
          "red": "#cc6666",
          "green": "#66cc66",
          "yellow": "#cccc66",
          "blue": "#6666cc",
          "purple": "#cc66cc",
          "cyan": "#66cccc",
          "white": "#e8f5e8",
          "brightBlack": "#3a4a3a",
          "brightRed": "#ff8888",
          "brightGreen": "#88ff88",
          "brightYellow": "#ffff88",
          "brightBlue": "#8888ff",
          "brightPurple": "#ff88ff",
          "brightCyan": "#88ffff",
          "brightWhite": "#ffffff"
        }"##,
    )
    .unwrap();
    // Reserved built-in id must not be overridden by a file.
    fs::write(themes.join("one-half-dark.json"), "{}").unwrap();

    // SAFETY: process_env_lock serializes RHO_HOME mutation for this test.
    unsafe {
        std::env::set_var("RHO_HOME", root.path());
    }

    let items = list_themes();
    let forest = items
        .iter()
        .find(|item| item.id() == "forest")
        .expect("custom forest theme");
    assert_eq!(forest.name(), "Forest");
    assert!(forest.is_custom());

    assert!(items.iter().any(|item| item.id() == "one-half-dark"));
    assert!(items.iter().any(|item| item.id() == TERMINAL_THEME_ID));
    // Alphabetical by name: One Half Dark before Forest before Terminal, etc.
    let names: Vec<_> = items.iter().map(|item| item.name()).collect();
    let mut sorted = names.clone();
    sorted.sort_by(|a, b| a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase()));
    assert_eq!(names, sorted);

    let loaded = resolve_fixed_scheme("forest").expect("load forest");
    assert_eq!(loaded.name, "Forest");
    assert_eq!(loaded.source, ThemeSourceKind::Custom);

    // Catalog entries already carry the scheme — no second resolve needed.
    match forest {
        ThemeEntry::Fixed(scheme) => assert_eq!(scheme.name, "Forest"),
        ThemeEntry::Terminal => panic!("forest should be fixed"),
    }

    unsafe {
        std::env::remove_var("RHO_HOME");
    }
}
