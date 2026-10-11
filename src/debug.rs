use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub struct Metrics {
    pub frame_ms: f32,
    pub fps: f32,
    pub average_frame_ms: f32,
    pub low_1_percent_fps: f32,
    pub cpu_ms: f32,
    pub loaded_chunks: usize,
    pub queued_chunks: usize,
    pub rendered_chunks: usize,
    pub vertices: usize,
    pub player_position: [f32; 3],
    pub player_velocity: [f32; 3],
    pub player_chunk: [i64; 3],
    pub local_position: [usize; 3],
    pub seed: u64,
    pub world_name: String,
    pub state: String,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            frame_ms: 0.0,
            fps: 0.0,
            average_frame_ms: 0.0,
            low_1_percent_fps: 0.0,
            cpu_ms: 0.0,
            loaded_chunks: 0,
            queued_chunks: 0,
            rendered_chunks: 0,
            vertices: 0,
            player_position: [0.0; 3],
            player_velocity: [0.0; 3],
            player_chunk: [0; 3],
            local_position: [0; 3],
            seed: 0,
            world_name: "No world".into(),
            state: "Main Menu".into(),
        }
    }
}

#[derive(Default)]
pub struct MetricsHistory {
    frames: VecDeque<f32>,
}

impl MetricsHistory {
    pub fn record(&mut self, frame_ms: f32) -> (f32, f32, f32) {
        self.frames.push_back(frame_ms.max(0.0));
        while self.frames.len() > 300 {
            self.frames.pop_front();
        }
        let average = self.frames.iter().sum::<f32>() / self.frames.len().max(1) as f32;
        let mut sorted = self.frames.iter().copied().collect::<Vec<_>>();
        sorted.sort_by(f32::total_cmp);
        let low_index =
            ((sorted.len() as f32 * 0.99).floor() as usize).min(sorted.len().saturating_sub(1));
        let low_fps = if sorted[low_index] > 0.0 {
            1000.0 / sorted[low_index]
        } else {
            0.0
        };
        let fps = if frame_ms > 0.0 {
            1000.0 / frame_ms
        } else {
            0.0
        };
        (fps, average, low_fps)
    }
}

pub fn recommendations(metrics: &Metrics) -> Vec<String> {
    let mut result = Vec::new();
    if metrics.frame_ms > 33.0 && metrics.cpu_ms > metrics.frame_ms * 0.7 {
        result.push(format!(
            "CPU frame time is {:.1} ms; inspect simulation, meshing, or streaming work.",
            metrics.cpu_ms
        ));
    }
    if metrics.queued_chunks > 12 {
        result.push(format!(
            "Chunk queue has {} jobs; generation throughput is below demand.",
            metrics.queued_chunks
        ));
    }
    if metrics.rendered_chunks > 200 && metrics.vertices > 1_000_000 {
        result.push(
            "Visible-face geometry is dense; profile chunk meshes before increasing view distance."
                .into(),
        );
    }
    result.truncate(3);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_is_bounded_and_recommendations_use_real_thresholds() {
        let mut history = MetricsHistory::default();
        for _ in 0..500 {
            history.record(10.0);
        }
        assert_eq!(history.frames.len(), 300);
        let metrics = Metrics {
            queued_chunks: 20,
            ..Default::default()
        };
        assert_eq!(recommendations(&metrics).len(), 1);
    }
}
