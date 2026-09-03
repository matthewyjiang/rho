//! Overlay frozen identity flags onto a freshly generated CLI argv.
//!
//! Permission-sensitive flags stay with the regenerated plan. Only flags the
//! caller lists (model, effort, max-turns, …) may be copied from frozen argv.

/// Copy listed identity flags from `frozen` onto `generated`.
///
/// Each flag is treated as a pair (`--flag`, value). Missing flags are
/// appended; existing values are replaced. Flags not in `identity_flags` are
/// ignored even if present on `frozen`.
pub(crate) fn overlay_identity_flags(
    mut generated: Vec<String>,
    frozen: &[String],
    identity_flags: &[&str],
) -> Vec<String> {
    for flag in identity_flags {
        if let Some(value) = single_flag_value(frozen, flag) {
            set_single_flag_value(&mut generated, flag, value);
        }
    }
    generated
}

fn single_flag_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1].clone())
}

fn set_single_flag_value(args: &mut Vec<String>, flag: &str, value: String) {
    if let Some(index) = args.iter().position(|arg| arg == flag) {
        if index + 1 < args.len() {
            args[index + 1] = value;
            return;
        }
    }
    args.push((*flag).to_string());
    args.push(value);
}

#[cfg(test)]
#[path = "frozen_args_tests.rs"]
mod tests;
