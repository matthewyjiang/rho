#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImageContent {
    pub data: String,
    pub mime_type: String,
}

pub fn image_summary(image: &ImageContent) -> String {
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &image.data)
        .map(|bytes| bytes.len())
        .unwrap_or_default();
    format!("{} {}", image.mime_type, format_bytes(bytes))
}

fn format_bytes(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}
