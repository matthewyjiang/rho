//! One completed tool result, with independent model and script views.

#[cfg(test)]
#[path = "rendered_tests.rs"]
mod tests;

use rho_sdk::{
    model::ImageContent,
    tool::{ToolAsset, ToolError, ToolErrorKind, ToolMetadata, ToolOutput},
};
use schemars::{generate::SchemaSettings, JsonSchema};
use serde::Serialize;
use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{Arc, LazyLock, OnceLock, RwLock},
};

/// Measure serialized JSON bytes without allocating an encoded copy.
pub fn json_len<T: Serialize + ?Sized>(value: &T) -> Result<usize, serde_json::Error> {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value)?;
    Ok(counter.0)
}

/// A completed result. Missing data means the tool has no script-facing value.
/// Structured data is bounded by its serialized JSON bytes via [`Self::limit_data`];
/// oversized data is omitted with an explicit truncation notice. Images and
/// assets are independent of the text and structured budgets.
#[derive(Debug, PartialEq)]
pub struct Rendered<T> {
    text: String,
    data: Option<T>,
    failed: bool,
    assets: Vec<ToolAsset>,
    images: Vec<ImageContent>,
}

impl<T> Rendered<T> {
    pub fn new(text: String, data: T) -> Self {
        Self {
            text,
            data: Some(data),
            failed: false,
            assets: Vec::new(),
            images: Vec::new(),
        }
    }

    pub fn text_only(text: String) -> Self {
        Self {
            text,
            data: None,
            failed: false,
            assets: Vec::new(),
            images: Vec::new(),
        }
    }

    pub fn failed_if(mut self, failed: bool) -> Self {
        self.failed = failed;
        self
    }

    pub fn with_assets(mut self, assets: Vec<ToolAsset>) -> Self {
        self.assets = assets;
        self
    }

    pub fn with_images(mut self, images: Vec<ImageContent>) -> Self {
        self.images = images;
        self
    }

    pub fn map_text(mut self, map: impl FnOnce(String) -> String) -> Self {
        self.text = map(self.text);
        self
    }

    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn data(&self) -> Option<&T> {
        self.data.as_ref()
    }
    pub fn is_failure(&self) -> bool {
        self.failed
    }
    pub fn assets(&self) -> &[ToolAsset] {
        &self.assets
    }
    pub fn images(&self) -> &[ImageContent] {
        &self.images
    }

    /// Legacy model-only callers do not serialize the typed script view.
    pub fn into_result(self, id: String) -> crate::tool::ToolResult {
        crate::tool::ToolResult {
            id,
            ok: !self.failed,
            content: self.text,
        }
    }
}

impl<T: Serialize> Rendered<T> {
    /// Omit oversized structured data instead of exposing an unbounded script view.
    pub fn limit_data(self, max_output_bytes: usize) -> Result<Self, ToolError> {
        if let Some(data) = &self.data {
            let asked = json_len(data)
                .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
            return Ok(self.limit_data_with_size(max_output_bytes, asked));
        }
        Ok(self)
    }

    /// Apply the data budget using a previously measured serialized JSON size.
    /// `asked` must describe the current data, measured with [`json_len`].
    pub fn limit_data_with_size(mut self, max_output_bytes: usize, asked: usize) -> Self {
        if self.data.is_some() && asked > max_output_bytes {
            self.data = None;
            self.text = crate::tool::truncate(
                    format!("[structured data truncated: max_output_bytes {max_output_bytes}, received {asked} bytes]\n{}", self.text),
                    max_output_bytes.saturating_sub(crate::tool::TRUNCATION_MARKER.len()),
                );
        }
        self
    }

    pub fn into_tool_output(self, mut metadata: ToolMetadata) -> Result<ToolOutput, ToolError> {
        for asset in self.assets {
            metadata = metadata.asset(asset);
        }
        let mut output = ToolOutput::text(self.text)
            .metadata(metadata)
            .with_images(self.images);
        if let Some(data) = self.data {
            output = output
                .with_structured_content(serde_json::to_value(data).map_err(|error| {
                    ToolError::new(ToolErrorKind::Execution, error.to_string())
                })?);
        }
        Ok(if self.failed { output.failed() } else { output })
    }
}

/// Schema for emitted values, including required nullable fields. Inline
/// subschemas keep script-facing returns free of definition indirection.
pub fn output_schema<T: JsonSchema>() -> serde_json::Value {
    // JsonSchema's identity contract distinguishes schemas, including generic
    // parameters, without imposing a new 'static bound on callers.
    type SchemaCell = Arc<OnceLock<serde_json::Value>>;
    static SCHEMAS: LazyLock<RwLock<HashMap<Cow<'static, str>, SchemaCell>>> =
        LazyLock::new(RwLock::default);
    let id = T::schema_id();
    let cached = SCHEMAS
        .read()
        .expect("output schema cache")
        .get(&id)
        .cloned();
    let cell = cached.unwrap_or_else(|| {
        SCHEMAS
            .write()
            .expect("output schema cache")
            .entry(id)
            .or_default()
            .clone()
    });
    cell.get_or_init(|| {
        let mut settings = SchemaSettings::draft2020_12().for_serialize();
        settings.inline_subschemas = true;
        let mut schema: serde_json::Value =
            settings.into_generator().into_root_schema_for::<T>().into();
        if let Some(object) = schema.as_object_mut() {
            object.remove("$schema");
            object.remove("title");
        }
        schema
    })
    .clone()
}
