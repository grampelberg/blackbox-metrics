use std::{
    collections::HashMap,
    hash::Hash,
    sync::{Arc, RwLock, atomic::Ordering},
};

use metrics::{Key, KeyName, Metadata, Recorder, SharedString, Unit};
use metrics_util::registry::{AtomicStorage, Registry};
use tokio::time;

use crate::{
    CounterValue, Error, GaugeValue, HistogramValue, MetricsRead, ReadKey,
    dump_guard::DumpGuard,
    sampler::{Sampler, SamplerOptions},
    snapshot::{MetricKind, MetricMetadata, Snapshot},
};

/// In-memory `metrics::Recorder`, useful for:
///
/// - Debugging - dump either the current state or a snapshot of it at any time.
///   Particularly useful during shutdown to know what has changed.
/// - Reporting - expose metrics to users. You can show the current state of the
///   system, such as rates in the UI.
/// - Testing - assert that a metric's value is correct, either immediately or
///   on a deadline.
#[derive(Clone, Debug)]
pub struct BlackboxRecorder {
    registry: Arc<Registry<Key, AtomicStorage>>,
    metadata: Arc<RwLock<HashMap<(MetricKind, Key), MetricMetadata>>>,
}

impl Default for BlackboxRecorder {
    fn default() -> Self {
        Self {
            registry: Arc::new(Registry::atomic()),
            metadata: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

// I've not implemented a `reset` method here. This is because I'd like to use
// `set_default_local_recorder` as long as it is possible. I can use `{}` scope
// blocks to handle the resets and get a new registry.
//
// Note: reset can be implemented by `retain_counters` with a callback that
// always returns false.
impl BlackboxRecorder {
    /// Returns all counter values currently known by the recorder.
    #[must_use]
    #[allow(clippy::mutable_key_type)]
    pub fn all_counters(&self) -> HashMap<Key, u64> {
        self.registry
            .get_counter_handles()
            .iter()
            .map(|(key, counter)| {
                (key.clone(), counter.load(Ordering::Relaxed))
            })
            .collect::<HashMap<_, _>>()
    }

    /// Waits up to one second for a key to match the expected value.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Deadline`] if the value did not match before timeout,
    /// or [`Error::NoKey`] if the key was never observed while polling.
    #[allow(clippy::future_not_send)]
    pub async fn assert<K: ReadKey>(
        &self,
        key: &K,
        value: K::Value,
    ) -> Result<(), Error> {
        on_deadline(|| {
            if self.get(key).ok_or(Error::NoKey)? == value {
                return Ok(true);
            }

            Ok(false)
        })
        .await?;

        Ok(())
    }

    /// Captures a point-in-time snapshot of all recorded metrics.
    #[must_use]
    #[allow(clippy::mutable_key_type)]
    pub fn snapshot(&self) -> Snapshot {
        let metadata = self
            .metadata
            .read()
            .expect("metadata lock poisoned")
            .clone();
        Snapshot::from_registry_with_metadata(&self.registry, metadata)
    }

    /// Creates a guard that prints a snapshot when dropped. This can be used to
    /// log the state of the system at shutdown. Note that `[Drop]` will only
    /// fire on graceful shutdown. If you're looking for a dump after `ctrl-c`,
    /// consider using a signal handler.
    #[must_use]
    pub fn dump(&self) -> DumpGuard {
        DumpGuard::new(self.clone())
    }

    /// Builds a sampler over this recorder with default options. To define more
    /// options, use `[Sampler]` directly.
    #[must_use]
    pub fn sampler<K>(&self, keys: Vec<K>) -> Sampler<Self, K>
    where
        K: ReadKey + Clone + Eq + Hash,
        K::Value: Clone,
    {
        Sampler::new(self.clone(), keys)
    }

    fn record_metadata(
        &self,
        kind: MetricKind,
        key: &Key,
        metadata: &Metadata<'_>,
    ) {
        let metadata = MetricMetadata::new(
            metadata.target().to_string(),
            *metadata.level(),
            metadata.module_path().map(str::to_string),
        );

        let mut lookup = self.metadata.write().expect("metadata lock poisoned");
        lookup.entry((kind, key.clone())).or_insert(metadata);
    }
}

impl MetricsRead for BlackboxRecorder {
    fn get_counter(&self, key: &Key) -> Option<CounterValue> {
        self.registry
            .get_counter(key)
            .map(|v| v.load(Ordering::Relaxed))
    }

    fn get_gauge(&self, key: &Key) -> Option<GaugeValue> {
        self.registry
            .get_gauge(key)
            .map(|v| f64::from_bits(v.load(Ordering::Relaxed)))
    }

    fn get_histogram(&self, key: &Key) -> Option<HistogramValue> {
        self.registry.get_histogram(key).map(|v| v.data())
    }
}

impl Recorder for BlackboxRecorder {
    fn describe_counter(
        &self,
        _key: KeyName,
        _unit: Option<Unit>,
        _description: SharedString,
    ) {
    }

    fn describe_gauge(
        &self,
        _key: KeyName,
        _unit: Option<Unit>,
        _description: SharedString,
    ) {
    }

    fn describe_histogram(
        &self,
        _key: KeyName,
        _unit: Option<Unit>,
        _description: SharedString,
    ) {
    }

    fn register_counter(
        &self,
        key: &Key,
        metadata: &Metadata<'_>,
    ) -> metrics::Counter {
        self.record_metadata(MetricKind::Counter, key, metadata);

        self.registry.get_or_create_counter(key, |c| {
            metrics::Counter::from_arc(c.clone())
        })
    }

    fn register_gauge(
        &self,
        key: &Key,
        metadata: &Metadata<'_>,
    ) -> metrics::Gauge {
        self.record_metadata(MetricKind::Gauge, key, metadata);
        self.registry
            .get_or_create_gauge(key, |g| metrics::Gauge::from_arc(g.clone()))
    }

    fn register_histogram(
        &self,
        key: &Key,
        metadata: &Metadata<'_>,
    ) -> metrics::Histogram {
        self.record_metadata(MetricKind::Histogram, key, metadata);
        self.registry.get_or_create_histogram(key, |h| {
            metrics::Histogram::from_arc(h.clone())
        })
    }
}

async fn on_deadline<F>(check: F) -> Result<(), Error>
where
    F: Fn() -> Result<bool, Error>,
{
    let deadline = time::Duration::from_secs(1);
    time::timeout(deadline, async move {
        let mut tick = time::interval(time::Duration::from_millis(10));
        loop {
            tick.tick().await;

            if check()? {
                break;
            }
        }

        Ok::<(), Error>(())
    })
    .await
    .map_err(|_| Error::Deadline)??;

    Ok(())
}
