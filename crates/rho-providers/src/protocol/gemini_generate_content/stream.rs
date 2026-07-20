use futures_util::StreamExt;

use crate::{
    model::{ModelError, ModelEvent, ModelResponse},
    provider_backend::{line_decoder::LineDecoder, stream_timeout::StreamIdleDeadline},
};

use super::{GenerateContentResponse, ResponseCollector};

pub async fn collect_stream(
    response: reqwest::Response,
    on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
) -> Result<ModelResponse, ModelError> {
    let mut collector = ResponseCollector::default();
    let mut lines = LineDecoder::default();
    let mut events = SseEventDecoder::default();
    let mut stream = response.bytes_stream();
    let mut idle = StreamIdleDeadline::new();
    loop {
        let Some(chunk) = idle
            .wait_for(stream.next())
            .await
            .map_err(|error| stream_error(&collector, error))?
        else {
            break;
        };
        let chunk = chunk
            .map_err(ModelError::from)
            .map_err(|error| stream_error(&collector, error))?;
        lines.push(&chunk);
        while let Some(line) = lines
            .next_line()
            .map_err(invalid_utf8)
            .map_err(|error| stream_error(&collector, error))?
        {
            if events
                .apply_line(line, &mut collector, on_event)
                .map_err(|error| stream_error(&collector, error))?
            {
                idle.record_activity();
            }
        }
    }
    if let Some(line) = lines
        .finish()
        .map_err(invalid_utf8)
        .map_err(|error| stream_error(&collector, error))?
    {
        events
            .apply_line(line, &mut collector, on_event)
            .map_err(|error| stream_error(&collector, error))?;
    }
    events
        .finish(&mut collector, on_event)
        .map_err(|error| stream_error(&collector, error))?;
    collector.finish()
}

#[derive(Default)]
struct SseEventDecoder {
    data: Vec<String>,
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
        self.data
            .push(data.strip_prefix(' ').unwrap_or(data).into());
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
        if self.data.is_empty() {
            return Ok(());
        }
        let data = self.data.join("\n");
        self.data.clear();
        if data.trim() == "[DONE]" {
            return Ok(());
        }
        let response: GenerateContentResponse = serde_json::from_str(&data).map_err(|error| {
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

fn invalid_utf8(error: std::str::Utf8Error) -> ModelError {
    ModelError::InvalidResponse(format!("invalid UTF-8 in Gemini stream: {error}"))
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
