//! Lightweight timing samples for opt-in harness measurements.

use std::time::Duration;

use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct TimingSample {
    pub name: String,
    pub duration_ms: u128,
}

impl TimingSample {
    pub fn new(name: impl Into<String>, duration: Duration) -> Self {
        Self {
            name: name.into(),
            duration_ms: duration.as_millis(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct TimingSummary {
    pub samples: Vec<TimingSample>,
}

impl TimingSummary {
    pub fn push(&mut self, sample: TimingSample) {
        self.samples.push(sample);
    }

    pub fn percentile_ms(&self, percentile: f64) -> Option<u128> {
        if self.samples.is_empty() {
            return None;
        }
        let mut values = self
            .samples
            .iter()
            .map(|sample| sample.duration_ms)
            .collect::<Vec<_>>();
        values.sort_unstable();
        let rank =
            ((percentile.clamp(0.0, 100.0) / 100.0) * ((values.len() - 1) as f64)).round() as usize;
        values.get(rank).copied()
    }

    pub fn report_lines(&self) -> Vec<String> {
        if self.samples.is_empty() {
            return vec!["timing: no samples".into()];
        }
        let mut lines = vec![format!("timing samples: {}", self.samples.len())];
        for sample in &self.samples {
            lines.push(format!("  {}={}ms", sample.name, sample.duration_ms));
        }
        if let (Some(p50), Some(p95), Some(p99)) = (
            self.percentile_ms(50.0),
            self.percentile_ms(95.0),
            self.percentile_ms(99.0),
        ) {
            lines.push(format!("  p50={p50}ms p95={p95}ms p99={p99}ms"));
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn computes_percentiles() {
        let mut summary = TimingSummary::default();
        for ms in [10, 20, 30, 40, 50] {
            summary.push(TimingSample {
                name: "x".into(),
                duration_ms: ms,
            });
        }
        assert_eq!(summary.percentile_ms(50.0), Some(30));
        assert_eq!(summary.percentile_ms(100.0), Some(50));
    }
}
