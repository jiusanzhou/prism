use std::collections::HashMap;
use std::sync::Mutex;

/// Accumulated outcome statistics per variant
#[derive(Debug, Clone, Default)]
pub struct VariantOutcomeStats {
    pub total_requests: u64,
    pub successful_requests: u64,
    pub total_latency_ms: u64,
    pub total_tokens: u64,
    pub total_cost: f64,
    pub user_ratings: Vec<u8>, // 1-5 ratings
    pub error_count: u64,
}

impl VariantOutcomeStats {
    pub fn success_rate(&self) -> f64 {
        if self.total_requests == 0 {
            return 0.0;
        }
        self.successful_requests as f64 / self.total_requests as f64
    }

    pub fn avg_latency_ms(&self) -> f64 {
        if self.total_requests == 0 {
            return 0.0;
        }
        self.total_latency_ms as f64 / self.total_requests as f64
    }

    pub fn avg_rating(&self) -> f64 {
        if self.user_ratings.is_empty() {
            return 0.0;
        }
        self.user_ratings.iter().map(|r| *r as f64).sum::<f64>() / self.user_ratings.len() as f64
    }

    pub fn avg_cost_per_request(&self) -> f64 {
        if self.total_requests == 0 {
            return 0.0;
        }
        self.total_cost / self.total_requests as f64
    }
}

/// Outcome report from the gateway
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct OutcomeReport {
    pub variant_id: String,
    pub success: bool,
    pub latency_ms: u64,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub cost: Option<f64>,
    pub user_rating: Option<u8>,
}

/// Internal record for windowed stats tracking
#[derive(Debug, Clone)]
struct OutcomeRecord {
    success: bool,
    latency_ms: u64,
    tokens: u64,
    cost: f64,
    user_rating: Option<u8>,
}

/// Per-variant windowed state
#[derive(Debug, Clone)]
struct VariantWindow {
    records: Vec<OutcomeRecord>,
    head: usize, // next write position (circular buffer)
    count: usize,
}

impl VariantWindow {
    fn new(window_size: usize) -> Self {
        Self {
            records: Vec::with_capacity(window_size),
            head: 0,
            count: 0,
        }
    }

    fn push(&mut self, record: OutcomeRecord, window_size: usize) {
        if self.records.len() < window_size {
            self.records.push(record);
        } else {
            self.records[self.head] = record;
        }
        self.head = (self.head + 1) % window_size;
        self.count += 1;
    }

    fn compute_stats(&self) -> VariantOutcomeStats {
        let mut stats = VariantOutcomeStats {
            total_requests: self.records.len() as u64,
            ..Default::default()
        };

        for rec in &self.records {
            if rec.success {
                stats.successful_requests += 1;
            } else {
                stats.error_count += 1;
            }
            stats.total_latency_ms += rec.latency_ms;
            stats.total_tokens += rec.tokens;
            stats.total_cost += rec.cost;
            if let Some(rating) = rec.user_rating {
                stats.user_ratings.push(rating);
            }
        }

        stats
    }
}

/// The feedback collector accumulates outcomes and provides quality signals
/// that can be used to adjust routing weights.
pub struct FeedbackCollector {
    windows: Mutex<HashMap<String, VariantWindow>>,
    /// Window size: only keep last N outcomes per variant for rolling stats
    window_size: usize,
}

impl FeedbackCollector {
    pub fn new(window_size: usize) -> Self {
        Self {
            windows: Mutex::new(HashMap::new()),
            window_size,
        }
    }

    /// Record an outcome
    pub fn record(&self, report: &OutcomeReport) {
        let tokens =
            report.prompt_tokens.unwrap_or(0) as u64 + report.completion_tokens.unwrap_or(0) as u64;

        let record = OutcomeRecord {
            success: report.success,
            latency_ms: report.latency_ms,
            tokens,
            cost: report.cost.unwrap_or(0.0),
            user_rating: report.user_rating,
        };

        let mut windows = self.windows.lock().unwrap();
        let window = windows
            .entry(report.variant_id.clone())
            .or_insert_with(|| VariantWindow::new(self.window_size));
        window.push(record, self.window_size);
    }

    /// Get stats for a variant
    pub fn get_stats(&self, variant_id: &str) -> Option<VariantOutcomeStats> {
        let windows = self.windows.lock().unwrap();
        windows.get(variant_id).map(|w| w.compute_stats())
    }

    /// Get all stats
    pub fn get_all_stats(&self) -> HashMap<String, VariantOutcomeStats> {
        let windows = self.windows.lock().unwrap();
        windows
            .iter()
            .map(|(k, v)| (k.clone(), v.compute_stats()))
            .collect()
    }

    /// Compute a quality adjustment factor for a variant based on feedback.
    /// Returns a multiplier in [0.5, 1.5] that can be applied to the routing score.
    /// - High success rate + good ratings → > 1.0 (boost)
    /// - Low success rate + bad ratings → < 1.0 (penalize)
    pub fn quality_adjustment(&self, variant_id: &str) -> f32 {
        let windows = self.windows.lock().unwrap();
        let window = match windows.get(variant_id) {
            Some(w) => w,
            None => return 1.0, // no data → neutral
        };

        if window.records.is_empty() {
            return 1.0;
        }

        let stats = window.compute_stats();

        // Need a minimum number of samples for meaningful adjustment
        if stats.total_requests < 5 {
            return 1.0;
        }

        // Success rate component: maps [0, 1] → [-0.25, 0.25]
        // Baseline expectation: 95% success rate is "normal" (0 adjustment)
        let success_rate = stats.success_rate();
        let success_adj = (success_rate - 0.95) * 0.5; // ±0.025 per 5% deviation

        // Rating component: maps [1, 5] → [-0.25, 0.25]
        let rating_adj = if !stats.user_ratings.is_empty() {
            let avg = stats.avg_rating();
            (avg - 3.0) / 8.0 // 3.0 is "neutral", range [-0.25, 0.25]
        } else {
            0.0
        };

        // Combined adjustment, centered at 1.0
        let adjustment = 1.0 + success_adj + rating_adj;

        // Clamp to [0.5, 1.5]
        (adjustment as f32).clamp(0.5, 1.5)
    }
}
