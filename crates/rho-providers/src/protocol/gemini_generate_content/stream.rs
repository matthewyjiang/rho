use crate::{
    model::{ModelError, ModelEvent, ModelResponse},
    provider_backend::line_stream::collect_line_stream,
};

use super::{GenerateContentResponse, ResponseCollector};

pub async fn collect_stream(
    response: reqwest::Response,
    on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
) -> Result<ModelResponse, ModelError> {
    let mut collector = ResponseCollector::default();
    let mut events = SseEventDecoder::default();
    collect_line_stream(response, line_decode_error, |line| {
        events.apply_line(line, &mut collector, on_event)
    })
    .await
    .map_err(|error| stream_error(&collector, error))?;
    events
        .finish(&mut collector, on_event)
        .map_err(|error| stream_error(&collector, error))?;
    collector.finish()
}

#[derive(Default)]
struct SseEventDecoder {
    buffer: String,
}

impl SseEventDecoder {
    fn apply_line(
        &mut self,
        line: &str,
        collector: &mut ResponseCollector,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
    ) -> Result<bool, ModelError> {
        if line.is_empty() {
            self.flush(collector, on_event)?;
            return Ok(false);
        }
        if line.starts_with(':') {
            return Ok(false);
        }
        let Some(data) = line.strip_prefix("data:") else {
            return Ok(false);
        };
        let payload = data.strip_prefix(' ').unwrap_or(data);
        if !self.buffer.is_empty() {
            self.buffer.push('\n');
        }
        self.buffer.push_str(payload);
        Ok(true)
    }

    fn finish(
        &mut self,
        collector: &mut ResponseCollector,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
    ) -> Result<(), ModelError> {
        self.flush(collector, on_event)
    }

    fn flush(
        &mut self,
        collector: &mut ResponseCollector,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
    ) -> Result<(), ModelError> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        if self.buffer.trim() == "[DONE]" {
            self.buffer.clear();
            return Ok(());
        }
        let response_result: Result<GenerateContentResponse, _> =
            serde_json::from_str(&self.buffer);
        self.buffer.clear();
        let response = response_result.map_err(|error| {
            ModelError::InvalidResponse(format!("invalid Gemini stream event: {error}"))
        })?;
        collector.apply(response, Some(on_event))
    }
}

fn stream_error(collector: &ResponseCollector, error: ModelError) -> ModelError {
    if collector.has_emitted_output() && !matches!(error, ModelError::Interrupted) {
        ModelError::StreamFailedAfterOutput {
            message: error.to_string(),
        }
    } else {
        error
    }
}

fn line_decode_error(error: crate::provider_backend::line_decoder::LineDecodeError) -> ModelError {
    ModelError::InvalidResponse(format!("could not decode Gemini stream: {error}"))
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
