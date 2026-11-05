use pretty_assertions::assert_eq;

use super::*;

// Covers: the version gate compares numerically (1.10 is newer than 1.5) and
// never calls an unparseable version too old.
// Owner: agy version parsing; the probe itself needs a real agy.
#[test]
fn version_gate_compares_numerically() {
    let cases = [
        ("1.4.12", Some(true)),
        ("1.5.0", Some(false)),
        ("1.10.0\n", Some(false)),
        ("2.0.0", Some(false)),
        ("1.5", None),
        ("dev", None),
    ];
    for (version, too_old) in cases {
        assert_eq!(
            parse_version(version).map(|parsed| parsed < MIN_VERSION),
            too_old,
            "{version}"
        );
    }
}

// Covers: only agy's own signed-out message reads as signed out; any other
// failure (network, crash, a different "sign in" error) is a probe failure,
// never a false "not signed in".
// Owner: agy sign-in probe parsing; spawning agy is not unit tested.
#[test]
fn models_probe_classifies_sign_in() {
    let cases = [
        (true, "gemini-4-argon-medium\tGemini 4 Argon (Medium)", ModelsProbe::SignedIn),
        (
            false,
            "Fetching available models...\nError: Please sign in to view available models. Launch the CLI without arguments to sign in.",
            ModelsProbe::SignedOut,
        ),
        (false, "Error: failed to sign in: network unreachable", ModelsProbe::Failed),
        (false, "", ModelsProbe::Failed),
    ];
    for (success, output, expected) in cases {
        assert_eq!(classify_models(success, output), expected, "{output}");
    }
}
