use std::{collections::HashMap, fmt, sync::atomic::Ordering};

use metrics::{Key, Level};
use metrics_util::registry::{AtomicStorage, Registry};

use crate::{CounterValue, GaugeValue, HistogramValue, MetricsRead};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
#[allow(clippy::redundant_pub_crate)]
pub(crate) enum MetricKind {
    Counter,
    Gauge,
    Histogram,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::redundant_pub_crate)]
pub(crate) struct MetricMetadata {
    target: String,
    level: Level,
    module_path: Option<String>,
}

impl MetricMetadata {
    #[allow(clippy::missing_const_for_fn)]
    pub(crate) fn new(
        target: String,
        level: Level,
        module_path: Option<String>,
    ) -> Self {
        Self {
            target,
            level,
            module_path,
        }
    }
}

/// A snapshot of all the metrics in a registry at a point in time.
///
/// # Example
///
/// ```rust
/// use blackbox_metrics::{BlackboxRecorder, KeyExt, MetricsRead};
/// use metrics::{counter, with_local_recorder};
///
/// let recorder = BlackboxRecorder::default();
///
/// with_local_recorder(&recorder, || {
///     counter!("requests_total").increment(2);
/// });
///
/// let snapshot = recorder.snapshot();
/// println!("{snapshot}");
/// assert_eq!(snapshot.get(&"requests_total".into_counter()), Some(2));
/// ```
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    counters: HashMap<Key, CounterValue>,
    gauges: HashMap<Key, GaugeValue>,
    histograms: HashMap<Key, HistogramValue>,
    metadata: HashMap<(MetricKind, Key), MetricMetadata>,
}

impl Snapshot {
    #[allow(clippy::mutable_key_type)]
    pub(crate) fn from_registry_with_metadata(
        registry: &Registry<Key, AtomicStorage>,
        metadata: HashMap<(MetricKind, Key), MetricMetadata>,
    ) -> Self {
        let mut this = Self {
            metadata,
            ..Self::default()
        };

        registry
            .get_counter_handles()
            .iter()
            .for_each(|(key, counter)| {
                this.counters
                    .insert(key.clone(), counter.load(Ordering::Relaxed));
            });

        registry
            .get_gauge_handles()
            .iter()
            .for_each(|(key, gauge)| {
                this.gauges.insert(
                    key.clone(),
                    f64::from_bits(gauge.load(Ordering::Relaxed)),
                );
            });

        registry
            .get_histogram_handles()
            .iter()
            .for_each(|(key, histogram)| {
                this.histograms.insert(key.clone(), histogram.data());
            });

        this
    }

    fn target_for(&self, kind: MetricKind, key: &Key) -> &str {
        self.metadata
            .get(&(kind, key.clone()))
            .map_or("-", |m| m.target.as_str())
    }

    fn row_for(&self, kind: MetricKind, key: &Key) -> Vec<Row> {
        let target = self.target_for(kind, key).to_string();
        let metric = key.to_output();

        match kind {
            MetricKind::Counter => self
                .counters
                .get(key)
                .map(|value| {
                    vec![Row::new(target, metric, None, value.to_string())]
                })
                .unwrap_or_default(),
            MetricKind::Gauge => self
                .gauges
                .get(key)
                .map(|value| {
                    vec![Row::new(target, metric, None, value.to_string())]
                })
                .unwrap_or_default(),
            MetricKind::Histogram => self
                .histograms
                .get(key)
                .map(|values| {
                    values
                        .iter()
                        .enumerate()
                        .map(|(idx, value)| {
                            Row::new(
                                target.clone(),
                                metric.clone(),
                                Some(idx),
                                value.to_string(),
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        }
    }
}

impl MetricsRead for Snapshot {
    fn get_counter(&self, key: &Key) -> Option<CounterValue> {
        self.counters.get(key).copied()
    }

    fn get_gauge(&self, key: &Key) -> Option<GaugeValue> {
        self.gauges.get(key).copied()
    }

    fn get_histogram(&self, key: &Key) -> Option<HistogramValue> {
        self.histograms.get(key).cloned()
    }
}

impl fmt::Display for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut rows =
            self.counters
                .iter()
                .flat_map(|(key, _)| self.row_for(MetricKind::Counter, key))
                .chain(
                    self.gauges.iter().flat_map(|(key, _)| {
                        self.row_for(MetricKind::Gauge, key)
                    }),
                )
                .chain(self.histograms.iter().flat_map(|(key, _)| {
                    self.row_for(MetricKind::Histogram, key)
                }))
                .collect::<Vec<_>>();

        rows.sort_unstable();

        let target_width =
            rows.iter().map(|row| row.target.len()).max().unwrap_or(0);
        let metric_width =
            rows.iter().map(|row| row.metric.len()).max().unwrap_or(0);

        write!(
            f,
            "{}",
            rows.iter()
                .map(|row| row.render(target_width, metric_width))
                .collect::<Vec<_>>()
                .join("\n")
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct Row {
    target: String,
    metric: String,
    idx: Option<usize>,
    value: String,
}

impl Row {
    #[allow(clippy::missing_const_for_fn)]
    fn new(
        target: String,
        metric: String,
        idx: Option<usize>,
        value: String,
    ) -> Self {
        Self {
            target,
            metric,
            idx,
            value,
        }
    }

    fn render(&self, target_width: usize, metric_width: usize) -> String {
        let metric = self.idx.map_or_else(
            || self.metric.clone(),
            |idx| format!("{}.{idx}", self.metric),
        );

        format!(
            "{:<target_width$} {:<metric_width$} {}",
            self.target,
            metric,
            self.value,
            target_width = target_width,
            metric_width = metric_width
        )
    }
}

trait Output {
    fn to_output(&self) -> String;
}

impl Output for Key {
    fn to_output(&self) -> String {
        let name = self.name().to_string();
        let mut labels = self
            .labels()
            .map(|label| (label.key().to_string(), label.value().to_string()))
            .collect::<Vec<_>>();
        labels.sort_unstable();

        if labels.is_empty() {
            return name;
        }

        let labels = labels
            .iter()
            .map(|(k, v)| format!(r#"{k}="{v}""#))
            .collect::<Vec<_>>()
            .join(",");

        format!("{name}{{{labels}}}")
    }
}
