use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    hash::Hash,
    sync::{Arc, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio::time;

use crate::{
    CounterKey, CounterValue, GaugeKey, HistogramKey, MetricsRead, ReadKey,
};

/// Configuration for sampler cadence and retained point window.
#[derive(Clone, Debug, bon::Builder)]
pub struct SamplerOptions {
    #[builder(default = Duration::from_secs(1))]
    interval: Duration,
    #[builder(default = 120)]
    capacity: usize,
}

impl Default for SamplerOptions {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(1),
            capacity: 120,
        }
    }
}

/// A sampled metric value at a Unix millisecond timestamp.
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct SamplePoint<V> {
    /// Timestamp in milliseconds since Unix epoch.
    pub at_ms: u64,
    /// Sampled value.
    pub value: V,
}

/// Derived counter statistics for a sampled series.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct CounterStats {
    /// Estimated rate-per-second over the sampled window.
    pub rate: Option<f64>,
    /// Latest total counter value.
    pub total: CounterValue,
}

/// A time series with derived statistics.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Series<T, S> {
    /// Sample points ordered from oldest to newest.
    pub points: Vec<SamplePoint<T>>,
    /// Aggregate or derived stats for `points`.
    pub stats: S,
}

type SampleQueue<V> = VecDeque<SamplePoint<V>>;
type SharedPoints<K, V> = Arc<RwLock<HashMap<K, SampleQueue<V>>>>;

/// Handle for querying sampled values.
#[derive(Clone, Debug)]
pub struct SamplerHandle<K, V>
where
    K: Clone + Eq + Hash,
    V: Clone,
{
    points: SharedPoints<K, V>,
}

impl<K, V> SamplerHandle<K, V>
where
    K: Clone + Eq + Hash,
    V: Clone,
{
    /// Returns the latest sampled point for `key`.
    #[must_use]
    pub fn latest(&self, key: &K) -> Option<SamplePoint<V>> {
        self.points
            .read()
            .expect("sampler lock poisoned")
            .get(key)
            .and_then(|points| points.back().cloned())
    }

    /// Returns all retained points per key.
    #[must_use]
    pub fn dump(&self) -> HashMap<K, Vec<SamplePoint<V>>> {
        self.points
            .read()
            .expect("sampler lock poisoned")
            .iter()
            .map(|(key, points)| {
                (key.clone(), points.iter().cloned().collect())
            })
            .collect()
    }
}

impl SamplerHandle<CounterKey, CounterValue> {
    /// Returns full series and basic stats for a counter.
    #[must_use]
    #[allow(clippy::significant_drop_tightening)]
    pub fn series(
        &self,
        key: &CounterKey,
    ) -> Option<Series<CounterValue, CounterStats>> {
        let guard = self.points.read().expect("sampler lock poisoned");
        let points = guard.get(key)?;
        let latest = points.back()?.clone();
        let points = points.iter().cloned().collect::<Vec<_>>();

        let rate = points
            .first()
            .and_then(|prev| Self::rate_from_prev(prev, &latest));

        Some(Series {
            points,
            stats: CounterStats {
                rate,
                total: latest.value,
            },
        })
    }

    /// Returns a rate between the two latest points for `key`.
    #[must_use]
    #[allow(clippy::significant_drop_tightening)]
    pub fn rate(&self, key: &CounterKey) -> Option<f64> {
        let guard = self.points.read().expect("sampler lock poisoned");
        let points = guard.get(key)?;
        let latest = points.back()?.clone();
        let prev = points.get(points.len().checked_sub(2)?)?.clone();

        Self::rate_from_prev(&prev, &latest)
    }

    /// Returns a rate from the earliest point in `window` to the latest point.
    #[must_use]
    #[allow(clippy::significant_drop_tightening)]
    pub fn rate_over(&self, key: &CounterKey, window: Duration) -> Option<f64> {
        let window_ms = u64::try_from(window.as_millis()).unwrap_or(u64::MAX);

        let guard = self.points.read().expect("sampler lock poisoned");
        let points = guard.get(key)?;
        let latest = points.back()?.clone();
        let cutoff_ms = latest.at_ms.saturating_sub(window_ms);
        let prev = points
            .iter()
            .find(|point| point.at_ms >= cutoff_ms)?
            .clone();

        Self::rate_from_prev(&prev, &latest)
    }

    #[allow(clippy::cast_precision_loss)]
    fn rate_from_prev(
        prev: &SamplePoint<CounterValue>,
        latest: &SamplePoint<CounterValue>,
    ) -> Option<f64> {
        if latest.at_ms <= prev.at_ms {
            return None;
        }

        let dt = Duration::from_millis(latest.at_ms - prev.at_ms).as_secs_f64();
        if dt <= 0.0 {
            return None;
        }

        let delta = latest.value.saturating_sub(prev.value) as f64;
        Some(delta / dt)
    }
}

/// Automatically samples a set of keys at regular intervals and reports on
/// statistics for them.
#[derive(Debug)]
pub struct Sampler<R, K>
where
    R: MetricsRead,
    K: ReadKey + Clone + Eq + Hash,
    K::Value: Clone,
{
    reader: R,
    keys: Vec<K>,
    options: SamplerOptions,
    points: SharedPoints<K, K::Value>,
}

impl<R, K> Sampler<R, K>
where
    R: MetricsRead,
    K: ReadKey + Clone + Eq + Hash,
    K::Value: Clone,
{
    /// Constructs a sampler with explicit options.
    #[must_use]
    pub fn new_with_opts(
        reader: R,
        keys: Vec<K>,
        options: SamplerOptions,
    ) -> Self {
        let capacity = options.capacity.max(1);
        let mut points = HashMap::new();
        keys.iter().cloned().for_each(|key| {
            points.insert(key, VecDeque::with_capacity(capacity));
        });

        Self {
            reader,
            keys,
            options,
            points: Arc::new(RwLock::new(points)),
        }
    }

    /// Constructs a sampler with default options.
    #[must_use]
    pub fn new(reader: R, keys: Vec<K>) -> Self {
        Self::new_with_opts(reader, keys, SamplerOptions::default())
    }

    #[allow(clippy::significant_drop_tightening)]
    fn sample(&self) {
        let at_ms = u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock before unix epoch")
                .as_millis(),
        )
        .unwrap_or(u64::MAX);

        self.keys.iter().cloned().for_each(|key| {
            if let Some(value) = self.reader.get(&key) {
                let mut map =
                    self.points.write().expect("sampler lock poisoned");
                let queue = map.entry(key).or_insert_with(|| {
                    VecDeque::with_capacity(self.options.capacity)
                });

                if queue.len() >= self.options.capacity {
                    queue.pop_front();
                }

                queue.push_back(SamplePoint { at_ms, value });
            }
        });
    }

    /// Convert this sampler into a handle that can be used to query it and a
    /// runner to collect the samples. You can start the runner, for example by:
    ///
    /// ```rust
    /// tokio::spawn(run);
    /// ```
    pub fn into_runner(
        self,
    ) -> (
        SamplerHandle<K, K::Value>,
        impl Future<Output = ()> + Send + 'static,
    )
    where
        R: Send + Sync + 'static,
        K: Send + Sync + 'static,
        K::Value: Send + Sync + 'static,
    {
        let handle = SamplerHandle {
            points: Arc::clone(&self.points),
        };

        let run = async move {
            let mut ticker = time::interval(self.options.interval);

            loop {
                ticker.tick().await;
                self.sample();
            }
        };

        (handle, run)
    }
}

/// Convenience alias for sampling counters.
pub type CounterSampler<R> = Sampler<R, CounterKey>;
/// Convenience alias for sampling gauges.
pub type GaugeSampler<R> = Sampler<R, GaugeKey>;
/// Convenience alias for sampling histograms.
pub type HistogramSampler<R> = Sampler<R, HistogramKey>;
