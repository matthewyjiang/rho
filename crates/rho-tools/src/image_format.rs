//! Shared image byte-signature helpers for tools and hosts.

/// Maximum image payload accepted for paste and tool preview paths.
pub const MAX_IMAGE_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// Detects PNG, JPEG, GIF, or WebP from leading magic bytes.
pub fn supported_image_mime_type(header: &[u8]) -> Option<&'static str> {
    if header.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if header.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if header.starts_with(b"RIFF") && header.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else {
        None
    }
}

#[cfg(test)]
#[path = "image_format_tests.rs"]
mod tests;
