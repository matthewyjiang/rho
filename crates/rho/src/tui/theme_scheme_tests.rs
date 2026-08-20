use super::*;
use pretty_assertions::assert_eq;
use std::fs;

// Covers: hex parse accepts #RGB and #RRGGBB; rejects non-hex
// Owner: tui theme scheme parse
#[test]
fn parses_hex_colors() {
    assert_eq!(Rgb::from_hex("#112233"), Some(Rgb::new(0x11, 0x22, 0x33)));
    assert_eq!(Rgb::from_hex("aabbcc"), Some(Rgb::new(0xaa, 0xbb, 0xcc)));
    assert_eq!(Rgb::from_hex("#abc"), Some(Rgb::new(0xaa, 0xbb, 0xcc)));
    assert_eq!(Rgb::from_hex("zz"), None);
    assert_eq!(Rgb::from_hex("+112233"), None);
    assert_eq!(Rgb::from_hex("-abc"), None);
    assert_eq!(Rgb::from_hex("12 34 56"), None);
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

// Covers: bad optional WT fields do not reject a scheme
// Owner: tui theme scheme parse
#[test]
fn ignores_invalid_optional_cursor_and_selection() {
    let json = r##"{
      "name": "Tolerant",
      "background": "#000000",
      "foreground": "#ffffff",
      "cursorColor": "not-a-color",
      "selectionBackground": "also-bad",
      "black": "#000000",
      "red": "#ff0000",
      "green": "#00ff00",
      "yellow": "#ffff00",
      "blue": "#0000ff",
      "purple": "#ff00ff",
      "cyan": "#00ffff",
      "white": "#ffffff",
      "brightBlack": "#808080",
      "brightRed": "#ff0000",
      "brightGreen": "#00ff00",
      "brightYellow": "#ffff00",
      "brightBlue": "#0000ff",
      "brightPurple": "#ff00ff",
      "brightCyan": "#00ffff",
      "brightWhite": "#ffffff"
    }"##;
    let file: WindowsTerminalScheme = serde_json::from_str(json).unwrap();
    let scheme = file
        .into_scheme("tolerant", ThemeSourceKind::Custom)
        .expect("optional fields must not fail the scheme");
    assert_eq!(scheme.name, "Tolerant");
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

// Covers: duplicate built-in ids would silently hide a scheme and reserve the stem
// Owner: tui theme scheme catalog
#[test]
fn builtin_catalog_ids_are_unique_and_reserved() {
    let schemes = builtin_schemes();
    let mut ids: Vec<_> = schemes.iter().map(|scheme| scheme.id.clone()).collect();
    ids.sort();
    let mut unique = ids.clone();
    unique.dedup();
    assert_eq!(ids, unique);

    for scheme in schemes {
        assert!(!is_terminal_theme_id(&scheme.id));
        let resolved = resolve_fixed_scheme(&scheme.id).expect("builtin resolves");
        assert_eq!(resolved.source, ThemeSourceKind::Builtin);
        assert_eq!(resolved.id, scheme.id);
    }
}

// Covers: custom themes load from an injected directory (no RHO_HOME mutation)
// Owner: tui theme scheme catalog
#[test]
fn lists_custom_themes_from_injected_dir() {
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
    // Malformed custom file is skipped, not fatal.
    fs::write(themes.join("broken.json"), "{not json").unwrap();

    let items = list_themes_in(Some(&themes));
    let forest = items
        .iter()
        .find(|item| item.id() == "forest")
        .expect("custom forest theme");
    assert_eq!(forest.name(), "Forest");
    assert!(forest.is_custom());

    assert!(items.iter().any(|item| item.id() == "one-half-dark"));
    assert!(items.iter().any(|item| item.id() == TERMINAL_THEME_ID));
    assert!(!items.iter().any(|item| item.id() == "broken"));
    // Alphabetical by name
    let names: Vec<_> = items.iter().map(|item| item.name()).collect();
    let mut sorted = names.clone();
    sorted.sort_by_key(|a| a.to_ascii_lowercase());
    assert_eq!(names, sorted);

    let loaded = resolve_fixed_scheme_in("forest", Some(&themes)).expect("load forest");
    assert_eq!(loaded.name, "Forest");
    assert_eq!(loaded.source, ThemeSourceKind::Custom);

    // Catalog entries already carry the scheme — no second resolve needed.
    match forest {
        ThemeEntry::Fixed(scheme) => assert_eq!(scheme.name, "Forest"),
        ThemeEntry::Terminal => panic!("forest should be fixed"),
    }
}
