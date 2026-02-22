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

impl SamplerOptions {
    pub fn interval(&self) -> Duration {
        self.interval
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct SamplePoint<V> {
    pub at_ms: u64,
    pub value: V,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct CounterStats {
    pub rate: Option<f64>,
    pub total: CounterValue,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Series<T, S> {
    pub points: Vec<SamplePoint<T>>,
    pub stats: S,
}

#[derive(Clone)]
pub struct SamplerHandle<K, V>
where
    K: Clone + Eq + Hash,
    V: Clone,
{
    points: Arc<RwLock<HashMap<K, VecDeque<SamplePoint<V>>>>>,
}

impl<K, V> SamplerHandle<K, V>
where
    K: Clone + Eq + Hash,
    V: Clone,
{
    pub fn latest(&self, key: &K) -> Option<SamplePoint<V>> {
        self.points
            .read()
            .expect("sampler lock poisoned")
            .get(key)
            .and_then(|points| points.back().cloned())
    }

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

    pub fn rate(&self, key: &CounterKey) -> Option<f64> {
        let guard = self.points.read().expect("sampler lock poisoned");
        let points = guard.get(key)?;

        let latest = points.back()?;
        let prev = points.get(points.len().checked_sub(2)?)?;
        Self::rate_from_prev(prev, latest)
    }

    pub fn rate_over(&self, key: &CounterKey, window: Duration) -> Option<f64> {
        let guard = self.points.read().expect("sampler lock poisoned");
        let points = guard.get(key)?;
        let latest = points.back()?;

        let window_ms = window.as_millis() as u64;
        let cutoff_ms = latest.at_ms.saturating_sub(window_ms);

        let prev = points.iter().find(|point| point.at_ms >= cutoff_ms)?;
        Self::rate_from_prev(prev, latest)
    }

    fn rate_from_prev(
        prev: &SamplePoint<CounterValue>,
        latest: &SamplePoint<CounterValue>,
    ) -> Option<f64> {
        if latest.at_ms <= prev.at_ms {
            return None;
        }

        let dt = (latest.at_ms - prev.at_ms) as f64 / 1000.0;
        if dt <= 0.0 {
            return None;
        }

        let delta = latest.value.saturating_sub(prev.value) as f64;
        Some(delta / dt)
    }
}

pub struct Sampler<R, K>
where
    R: MetricsRead,
    K: ReadKey + Clone + Eq + Hash,
    K::Value: Clone,
{
    reader: R,
    keys: Vec<K>,
    options: SamplerOptions,
    points: Arc<RwLock<HashMap<K, VecDeque<SamplePoint<K::Value>>>>>,
}

impl<R, K> Sampler<R, K>
where
    R: MetricsRead,
    K: ReadKey + Clone + Eq + Hash,
    K::Value: Clone,
{
    pub fn new_with_opts(
        reader: R,
        keys: Vec<K>,
        options: SamplerOptions,
    ) -> Self {
        let capacity = options.capacity().max(1);
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

    pub fn new(reader: R, keys: Vec<K>) -> Self {
        Self::new_with_opts(reader, keys, SamplerOptions::default())
    }

    pub fn sample(&self) {
        let at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before unix epoch")
            .as_millis() as u64;

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

pub type CounterSampler<R> = Sampler<R, CounterKey>;
pub type GaugeSampler<R> = Sampler<R, GaugeKey>;
pub type HistogramSampler<R> = Sampler<R, HistogramKey>;
