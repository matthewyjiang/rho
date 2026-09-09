use std::collections::BTreeMap;

use pretty_assertions::assert_eq;

use super::{desktop_environment_from, driver_environment_from, telemetry_environment};

// Covers: explicit backend choices and compositor identity must survive
// forwarding, while credentials and unrelated driver controls stay excluded.
// Owner: computer-use child environment policy, with injected parent variables.
#[test]
fn desktop_environment_preserves_explicit_backend_selection_only() {
    for (present, forwarded) in [
        (
            vec![
                "DISPLAY",
                "WAYLAND_DISPLAY",
                "XDG_RUNTIME_DIR",
                "HYPRLAND_INSTANCE_SIGNATURE",
                "CUA_DRIVER_RS_ENABLE_WAYLAND",
                "OPENAI_API_KEY",
                "CUA_DRIVER_RS_UNRESTRICTED",
            ],
            vec![
                "DISPLAY",
                "WAYLAND_DISPLAY",
                "XDG_RUNTIME_DIR",
                "HYPRLAND_INSTANCE_SIGNATURE",
                "CUA_DRIVER_RS_ENABLE_WAYLAND",
            ],
        ),
        (vec!["WAYLAND_DISPLAY"], vec!["WAYLAND_DISPLAY"]),
        (vec!["DISPLAY", "XAUTHORITY"], vec!["DISPLAY", "XAUTHORITY"]),
        (vec!["OPENAI_API_KEY"], vec![]),
    ] {
        let actual = desktop_environment_from(|name| present.contains(&name));
        let expected: BTreeMap<_, _> = forwarded
            .into_iter()
            .map(|name| (name.to_owned(), name.to_owned()))
            .collect();
        assert_eq!(actual, expected, "parent variable names: {present:?}");
    }
}

// Covers: native Wayland defaults must not override explicit choices or affect X11.
// Owner: managed driver environment, using injected parent values.
#[test]
fn native_wayland_default_respects_session_and_override() {
    for (display, choice, expected_choice) in [
        (Some("wayland-1"), None, Some("1")),
        (Some("wayland-1"), Some("0"), Some("0")),
        (Some("wayland-1"), Some("false"), Some("false")),
        (Some("wayland-1"), Some(""), Some("")),
        (Some("wayland-1"), Some("1"), Some("1")),
        (Some(""), None, None),
        (None, None, None),
        (None, Some("1"), Some("1")),
    ] {
        let parent = BTreeMap::from([
            ("DISPLAY", Some(":0")),
            ("WAYLAND_DISPLAY", display),
            ("CUA_DRIVER_RS_ENABLE_WAYLAND", choice),
        ]);
        let mut actual =
            driver_environment_from(|name| parent.get(name).copied().flatten().map(Into::into));
        // Apply the same explicit-forwarding precedence as the MCP child.
        for (name, variable) in
            desktop_environment_from(|name| parent.get(name).is_some_and(Option::is_some))
        {
            actual.insert(name, parent[variable.as_str()].unwrap().into());
        }
        let mut expected = telemetry_environment();
        expected.insert("DISPLAY".into(), ":0".into());
        if let Some(display) = display {
            expected.insert("WAYLAND_DISPLAY".into(), display.into());
        }
        if let Some(choice) = expected_choice {
            expected.insert("CUA_DRIVER_RS_ENABLE_WAYLAND".into(), choice.into());
        }
        assert_eq!(actual, expected, "display: {display:?}, choice: {choice:?}");
    }
}
