//! `/codemode [status|on|only]`: Pi's `codemode.mode` for the always-registered
//! `codemode` tool.
//!
//! `on` declares direct tools next to `codemode`; `only` hides them by default.
//! Top-level `tool_search` can load direct tools in either mode. `status` or
//! bare `/codemode` reports the mode. There is no mode that removes `codemode`
//! and no `yolo` level: nested calls
//! inherit the session permission mode, so `/permissions` is the only
//! permission ladder.

use rho_protocol::{SettingChange, SettingKey, SettingValue};

use crate::config::CodemodeMode;

use super::{daemon_settings::SettingsNamespace, App, Entry};

/// The daemon setting `/codemode` reads and writes.
const CODEMODE_KEY: &str = "codemode.mode";

const CODEMODE_USAGE: &str = "usage: /codemode [status|on|only]";

impl App {
    /// `/codemode [status|on|only]` in a daemon session: the mode is the
    /// daemon's `codemode.mode` setting, which its sessions take at their
    /// next turn.
    pub(super) fn execute_daemon_codemode_command(&mut self, args: &str) {
        let key = SettingKey::new(CODEMODE_KEY);
        let Some(settings) = &self.daemon_settings else {
            self.insert_entry(&Entry::Error(
                "could not read codemode: the rho settings are not loaded".into(),
            ));
            return;
        };
        let saved = match settings.value(SettingsNamespace::Agent, &key) {
            Some(SettingValue::Text(mode)) => CodemodeMode::parse(mode),
            _ => None,
        };
        let args = args.trim();
        if args.is_empty() || args.eq_ignore_ascii_case("status") {
            match saved {
                Some(mode) => self.report_codemode(mode),
                None => self.set_status("codemode mode unknown"),
            }
            return;
        }
        let Some(requested) = CodemodeMode::parse(args) else {
            self.insert_entry(&Entry::Error(CODEMODE_USAGE.into()));
            self.set_status("invalid codemode mode");
            return;
        };
        if saved == Some(requested) {
            self.report_codemode(requested);
            return;
        }
        self.write_setting(
            SettingsNamespace::Agent,
            SettingChange::Set {
                key,
                value: SettingValue::Text(requested.as_str().into()),
            },
        );
    }

    fn report_codemode(&mut self, mode: CodemodeMode) {
        let status = match mode {
            CodemodeMode::On => "codemode on: direct tools and codemode are both available",
            CodemodeMode::Only => "codemode only: tools use codemode unless loaded by tool_search",
        };
        self.set_status(status);
    }
}
