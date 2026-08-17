//! Shared image byte-signature helpers for tools and hosts.

/// Maximum image payload accepted for paste and tool preview paths.
pub const MAX_IMAGE_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// Detects PNG, JPEG, GIF, or WebP from leading magic bytes.
pub fn supported_image_mime_type(header: &[u8]) -> Option<&'static str> {
    rho_sdk::model::ImageContent::mime_type_from_bytes(header)
}

#[cfg(test)]
#[path = "image_format_tests.rs"]
mod tests;
