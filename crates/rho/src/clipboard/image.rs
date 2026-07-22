use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use rho_providers::model::ImageContent;

use super::{
    process::{command_available, command_output},
    session::SessionKind,
};

#[derive(Debug, thiserror::Error)]
pub enum ClipboardImageError {
    #[error("no supported image found on clipboard")]
    NoImage,
    #[error("image file exceeds the {0} byte paste limit")]
    TooLarge(u64),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

const SUPPORTED_IMAGE_MIME_TYPES: &[&str] = &["image/png", "image/jpeg", "image/webp", "image/gif"];
const MAX_PASTE_IMAGE_FILE_BYTES: u64 = 32 * 1024 * 1024;

pub fn read_clipboard_image() -> Result<ImageContent, ClipboardImageError> {
    read_clipboard_image_for_session(SessionKind::detect())
}

/// Loads a PNG, JPEG, GIF, or WebP file as pasteable image content.
///
/// Hosts such as Herdr paste clipboard images as filesystem paths. Rho treats a
/// single-line paste of an image path as an attachment instead of plain text.
pub fn read_image_file(path: &Path) -> Result<ImageContent, ClipboardImageError> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(ClipboardImageError::NoImage);
    }
    if metadata.len() > MAX_PASTE_IMAGE_FILE_BYTES {
        return Err(ClipboardImageError::TooLarge(MAX_PASTE_IMAGE_FILE_BYTES));
    }
    let bytes = fs::read(path)?;
    image_content_from_bytes(bytes)
}

/// When `text` is only a path to an existing supported image, load it.
pub fn image_from_paste_text(text: &str, cwd: &Path) -> Option<ImageContent> {
    let path = paste_text_as_image_path(text, cwd)?;
    read_image_file(&path).ok()
}

pub(super) fn paste_text_as_image_path(text: &str, cwd: &Path) -> Option<PathBuf> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.contains('\n') || trimmed.contains('\r') {
        return None;
    }
    let unquoted = strip_matching_quotes(trimmed);
    if unquoted.is_empty() {
        return None;
    }
    let candidate = PathBuf::from(unquoted);
    let path = if candidate.is_absolute() {
        candidate
    } else {
        cwd.join(candidate)
    };
    path.canonicalize().ok().filter(|path| path.is_file())
}

fn strip_matching_quotes(text: &str) -> &str {
    let bytes = text.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &text[1..text.len() - 1];
        }
    }
    text
}

fn image_content_from_bytes(bytes: Vec<u8>) -> Result<ImageContent, ClipboardImageError> {
    if bytes.is_empty() {
        return Err(ClipboardImageError::NoImage);
    }
    let Some(mime_type) = supported_image_mime_type(&bytes) else {
        return Err(ClipboardImageError::NoImage);
    };
    Ok(ImageContent {
        data: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes),
        mime_type: mime_type.into(),
    })
}

fn supported_image_mime_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else {
        None
    }
}

pub(super) fn read_clipboard_image_for_session(
    session: SessionKind,
) -> Result<ImageContent, ClipboardImageError> {
    let image = match session {
        // Remote sessions have no path to the user's local image clipboard.
        SessionKind::Remote => None,
        SessionKind::Wsl => read_linux_clipboard_image().or_else(read_wsl_clipboard_image),
        SessionKind::Local if cfg!(target_os = "linux") => read_linux_clipboard_image(),
        SessionKind::Local if cfg!(target_os = "macos") => read_macos_clipboard_image(),
        SessionKind::Local if cfg!(target_os = "windows") => read_windows_clipboard_image(),
        SessionKind::Local => None,
    };

    let Some((bytes, mime_type)) = image else {
        return Err(ClipboardImageError::NoImage);
    };
    if bytes.is_empty() {
        return Err(ClipboardImageError::NoImage);
    }

    Ok(ImageContent {
        data: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes),
        mime_type,
    })
}

pub(super) fn available_image_helpers(session: SessionKind) -> Vec<&'static str> {
    available_image_helpers_with(session, command_available)
}

pub(super) fn available_image_helpers_with(
    session: SessionKind,
    host_command_available: impl Fn(&str) -> bool,
) -> Vec<&'static str> {
    match session {
        SessionKind::Remote => Vec::new(),
        // WSL is always a Linux guest, even when these unit tests compile on
        // macOS or Windows CI hosts.
        SessionKind::Wsl => {
            let mut helpers = ["wl-paste", "xclip"]
                .into_iter()
                .filter(|command| host_command_available(command))
                .collect::<Vec<_>>();
            if host_command_available("powershell.exe") {
                helpers.push("powershell.exe");
            }
            helpers
        }
        SessionKind::Local => platform_image_helpers()
            .iter()
            .copied()
            .filter(|command| host_command_available(command))
            .collect(),
    }
}

fn platform_image_helpers() -> &'static [&'static str] {
    if cfg!(target_os = "linux") {
        &["wl-paste", "xclip"]
    } else if cfg!(target_os = "macos") {
        &["pngpaste"]
    } else if cfg!(target_os = "windows") {
        &["powershell.exe"]
    } else {
        &[]
    }
}

fn read_linux_clipboard_image() -> Option<(Vec<u8>, String)> {
    read_clipboard_image_via_wl_paste().or_else(read_clipboard_image_via_xclip)
}

fn read_clipboard_image_via_wl_paste() -> Option<(Vec<u8>, String)> {
    let types = command_output("wl-paste", &["--list-types"])?;
    let selected_type = select_preferred_image_mime_type(&String::from_utf8_lossy(&types))?;
    let data = command_output("wl-paste", &["--type", &selected_type, "--no-newline"])?;
    Some((data, base_mime_type(&selected_type)))
}

fn read_clipboard_image_via_xclip() -> Option<(Vec<u8>, String)> {
    let candidate_types =
        command_output("xclip", &["-selection", "clipboard", "-t", "TARGETS", "-o"])
            .map(|types| String::from_utf8_lossy(&types).into_owned())
            .unwrap_or_default();
    let mut try_types = Vec::new();
    if let Some(preferred) = select_preferred_image_mime_type(&candidate_types) {
        try_types.push(preferred);
    }
    try_types.extend(
        SUPPORTED_IMAGE_MIME_TYPES
            .iter()
            .map(|mime| (*mime).to_string()),
    );

    for mime_type in try_types {
        if let Some(data) = command_output(
            "xclip",
            &["-selection", "clipboard", "-t", &mime_type, "-o"],
        )
        .filter(|data| !data.is_empty())
        {
            return Some((data, base_mime_type(&mime_type)));
        }
    }
    None
}

fn read_wsl_clipboard_image() -> Option<(Vec<u8>, String)> {
    let tmp_path = env::temp_dir().join(format!("rho-wsl-clip-{}.png", uuid::Uuid::new_v4()));
    let win_path = command_output("wslpath", &["-w", tmp_path.to_str()?])
        .and_then(|output| String::from_utf8(output).ok())?;
    let win_path = win_path.trim();
    if win_path.is_empty() {
        return None;
    }
    let ok = save_windows_clipboard_image_to(PathBuf::from(win_path), "powershell.exe")?;
    if !ok {
        return None;
    }
    let bytes = fs::read(&tmp_path).ok()?;
    let _ = fs::remove_file(&tmp_path);
    Some((bytes, "image/png".into()))
}

fn read_windows_clipboard_image() -> Option<(Vec<u8>, String)> {
    let tmp_path = env::temp_dir().join(format!("rho-clip-{}.png", uuid::Uuid::new_v4()));
    let ok = save_windows_clipboard_image_to(tmp_path.clone(), "powershell.exe")?;
    if !ok {
        return None;
    }
    let bytes = fs::read(&tmp_path).ok()?;
    let _ = fs::remove_file(&tmp_path);
    Some((bytes, "image/png".into()))
}

fn save_windows_clipboard_image_to(path: PathBuf, powershell: &str) -> Option<bool> {
    let path = path.to_string_lossy().replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; Add-Type -AssemblyName System.Drawing; \
         $img = [System.Windows.Forms.Clipboard]::GetImage(); \
         if ($img) {{ $img.Save('{path}', [System.Drawing.Imaging.ImageFormat]::Png); Write-Output 'ok' }} else {{ Write-Output 'empty' }}"
    );
    let output = command_output(powershell, &["-NoProfile", "-Command", &script])?;
    Some(String::from_utf8_lossy(&output).trim() == "ok")
}

fn read_macos_clipboard_image() -> Option<(Vec<u8>, String)> {
    let tmp_path = env::temp_dir().join(format!("rho-clip-{}.png", uuid::Uuid::new_v4()));
    let status = Command::new("pngpaste")
        .arg(&tmp_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    let bytes = fs::read(&tmp_path).ok()?;
    let _ = fs::remove_file(&tmp_path);
    Some((bytes, "image/png".into()))
}

fn select_preferred_image_mime_type(types: &str) -> Option<String> {
    let normalized = types
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|raw| (raw.to_string(), base_mime_type(raw)))
        .collect::<Vec<_>>();

    for preferred in SUPPORTED_IMAGE_MIME_TYPES {
        if let Some((raw, _)) = normalized.iter().find(|(_, base)| base == preferred) {
            return Some(raw.clone());
        }
    }
    None
}

fn base_mime_type(mime_type: &str) -> String {
    mime_type
        .split(';')
        .next()
        .unwrap_or(mime_type)
        .trim()
        .to_ascii_lowercase()
}

#[cfg(test)]
#[path = "image_tests.rs"]
mod tests;
