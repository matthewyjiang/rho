use pretty_assertions::assert_eq;

use super::*;

fn reader<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl FnMut(&str) -> Option<String> + 'a {
    move |name| {
        pairs
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| (*value).to_owned())
    }
}

#[test]
fn the_child_gets_the_base_set_plus_its_allowlist_and_nothing_else() {
    let ambient = [
        ("PATH", "/usr/bin"),
        ("MY_HOOK_TOKEN", "token"),
        ("ANTHROPIC_API_KEY", "secret"),
        ("UNRELATED", "value"),
    ];

    let environment = child_environment(&["MY_HOOK_TOKEN".to_owned()], reader(&ambient));

    assert_eq!(
        environment.get("PATH").map(String::as_str),
        Some("/usr/bin")
    );
    assert_eq!(
        environment.get("MY_HOOK_TOKEN").map(String::as_str),
        Some("token")
    );
    assert_eq!(environment.get("ANTHROPIC_API_KEY"), None);
    assert_eq!(environment.get("UNRELATED"), None);
}

#[test]
fn a_variable_missing_from_the_parent_is_simply_absent() {
    let environment = child_environment(&["NOT_SET".to_owned()], reader(&[("PATH", "/usr/bin")]));

    assert_eq!(environment.get("NOT_SET"), None);
    assert!(environment.contains_key("PATH"));
}

#[test]
fn the_recursion_marker_is_always_set() {
    let environment = child_environment(&[], reader(&[]));

    assert_eq!(
        environment
            .get(super::super::IN_HOOK_ENV)
            .map(String::as_str),
        Some("1")
    );
}

#[test]
fn an_allowlist_cannot_override_the_recursion_marker() {
    let environment = child_environment(
        &[super::super::IN_HOOK_ENV.to_owned()],
        reader(&[(super::super::IN_HOOK_ENV, "0")]),
    );

    assert_eq!(
        environment
            .get(super::super::IN_HOOK_ENV)
            .map(String::as_str),
        Some("1")
    );
}
