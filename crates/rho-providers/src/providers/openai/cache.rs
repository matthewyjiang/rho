use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sha2::{Digest, Sha256};

const PROMPT_CACHE_KEY_PREFIX: &str = "rho:";
const MAX_PROMPT_CACHE_KEY_LEN: usize = 64;
const HASH_LEN: usize = 43;

/// Builds a stable, provider-safe OpenAI prompt cache key from a rho session id.
///
/// The key is intentionally scoped to rho and clamped to a conservative length
/// so callers can pass full session ids without leaking arbitrary long strings
/// into provider request metadata.
pub fn prompt_cache_key_from_session_id(session_id: &str) -> Option<String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return None;
    }

    let sanitized = sanitize_prompt_cache_key_fragment(session_id);
    let full_key = format!("{PROMPT_CACHE_KEY_PREFIX}{sanitized}");
    if full_key.len() <= MAX_PROMPT_CACHE_KEY_LEN {
        return Some(full_key);
    }

    let hash = URL_SAFE_NO_PAD.encode(Sha256::digest(session_id.as_bytes()));
    let hash = &hash[..HASH_LEN];
    let available_prefix_len =
        MAX_PROMPT_CACHE_KEY_LEN - PROMPT_CACHE_KEY_PREFIX.len() - 1 - hash.len();
    let sanitized_prefix = sanitized
        .chars()
        .take(available_prefix_len)
        .collect::<String>()
        .trim_matches('-')
        .to_string();

    if sanitized_prefix.is_empty() {
        Some(format!("{PROMPT_CACHE_KEY_PREFIX}{hash}"))
    } else {
        Some(format!(
            "{PROMPT_CACHE_KEY_PREFIX}{sanitized_prefix}-{hash}"
        ))
    }
}

fn sanitize_prompt_cache_key_fragment(value: &str) -> String {
    let mut sanitized = String::new();
    let mut last_was_separator = false;

    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':') {
            sanitized.push(ch);
            last_was_separator = false;
        } else if !last_was_separator {
            sanitized.push('-');
            last_was_separator = true;
        }
    }

    let sanitized = sanitized.trim_matches('-');
    if sanitized.is_empty() {
        URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes()))[..HASH_LEN].to_string()
    } else {
        sanitized.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_uuid_maps_to_stable_prefixed_key() {
        let session_id = "123e4567-e89b-12d3-a456-426614174000";

        assert_eq!(
            prompt_cache_key_from_session_id(session_id).as_deref(),
            Some("rho:123e4567-e89b-12d3-a456-426614174000")
        );
    }

    #[test]
    fn cache_key_sanitizes_unsupported_characters() {
        assert_eq!(
            prompt_cache_key_from_session_id(" project / session 🚀 ").as_deref(),
            Some("rho:project-session")
        );
    }

    #[test]
    fn cache_key_is_clamped_and_stable_for_long_session_ids() {
        let session_id = "workspace/".repeat(20);
        let key = prompt_cache_key_from_session_id(&session_id).unwrap();

        assert!(key.starts_with("rho:workspace-work"));
        assert!(key.len() <= MAX_PROMPT_CACHE_KEY_LEN);
        assert_eq!(prompt_cache_key_from_session_id(&session_id).unwrap(), key);
        assert_ne!(
            prompt_cache_key_from_session_id(&(session_id + "x")).unwrap(),
            key
        );
    }

    #[test]
    fn blank_session_id_does_not_create_cache_key() {
        assert_eq!(prompt_cache_key_from_session_id("  "), None);
    }
}
