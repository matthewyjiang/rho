use pretty_assertions::assert_eq;

use super::AGENT_SETTINGS;
use crate::config::default_fits_kind;

// Covers: two rows on one key would load and save the same TOML location
// twice, so the later row silently wins.
// Owner: config registry
#[test]
fn every_agent_setting_has_its_own_key() {
    let mut keys = AGENT_SETTINGS
        .iter()
        .map(|setting| setting.key())
        .collect::<Vec<_>>();
    keys.sort_unstable();
    let declared = keys.len();
    keys.dedup();
    assert_eq!(keys.len(), declared);
}

// Covers: a declared range or choice list that rejects the setting's own
// default would make a fresh config fail to load once saved.
// Owner: config registry
#[test]
fn every_agent_default_fits_its_kind() {
    assert_eq!(
        AGENT_SETTINGS
            .iter()
            .map(|setting| setting.spec())
            .filter(|spec| !default_fits_kind(spec))
            .map(|spec| spec.key.to_string())
            .collect::<Vec<_>>(),
        Vec::<String>::new()
    );
}
