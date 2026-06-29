use std::{
    env, fs,
    path::PathBuf,
    process::{Command, Stdio},
};

use crate::model::ImageContent;

#[derive(Debug, thiserror::Error)]
pub enum ClipboardImageError {
    #[error("no supported image found on clipboard")]
    NoImage,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

const SUPPORTED_IMAGE_MIME_TYPES: &[&str] = &["image/png", "image/jpeg", "image/webp", "image/gif"];

pub fn read_clipboard_image() -> Result<ImageContent, ClipboardImageError> {
    let image = if cfg!(target_os = "linux") {
        read_linux_clipboard_image().or_else(|| is_wsl().then(read_wsl_clipboard_image).flatten())
    } else if cfg!(target_os = "macos") {
        read_macos_clipboard_image()
    } else if cfg!(target_os = "windows") {
        read_windows_clipboard_image()
    } else {
        None
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
    let ok = save_windows_clipboard_image_to(tmp_path.clone(), "powershell")?;
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

fn command_output(command: &str, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new(command)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
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

fn is_wsl() -> bool {
    env::var_os("WSL_DISTRO_NAME").is_some()
        || env::var_os("WSLENV").is_some()
        || fs::read_to_string("/proc/version")
            .map(|version| version.to_ascii_lowercase().contains("microsoft"))
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::select_preferred_image_mime_type;

    #[test]
    fn selects_only_supported_image_mime_types() {
        assert_eq!(
            select_preferred_image_mime_type("image/tiff\nimage/jpeg"),
            Some("image/jpeg".into())
        );
        assert_eq!(select_preferred_image_mime_type("image/tiff"), None);
    }
}
