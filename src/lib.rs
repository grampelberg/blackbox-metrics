//! A local, in-process metrics recorder for debugging, reporting and testing.

mod dump_guard;
mod recorder;
/// Automatically sample metric values to understand how they change over time.
/// This can calculate the rate of change of your counters, for example.
pub mod sampler;
mod snapshot;

use metrics::Key;

pub use crate::{recorder::BlackboxRecorder, snapshot::Snapshot};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Errors returned by recorder helpers.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Deadline elapsed before a condition became true.
    #[error("deadline exceeded")]
    Deadline,

    /// Wrapped metrics subsystem error.
    #[error("metrics: {0}")]
    Metrics(BoxError),

    /// Metric key has not been observed/registered yet.
    #[error("key has not been registered")]
    NoKey,
}

/// Counter value type.
pub type CounterValue = u64;

/// Gauge value type.
pub type GaugeValue = f64;

/// Histogram value type.
pub type HistogramValue = Vec<f64>;

/// Newtype for counter metric keys.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct CounterKey(Key);

impl CounterKey {
    /// Returns the wrapped `metrics::Key`.
    pub const fn as_key(&self) -> &Key {
        &self.0
    }
}

/// Newtype for gauge metric keys.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct GaugeKey(Key);

impl GaugeKey {
    /// Returns the wrapped `metrics::Key`.
    pub const fn as_key(&self) -> &Key {
        &self.0
    }
}

/// Newtype for histogram metric keys.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct HistogramKey(Key);

impl HistogramKey {
    /// Returns the wrapped `metrics::Key`.
    pub const fn as_key(&self) -> &Key {
        &self.0
    }
}

/// Conversion from Key to the typed wrappers.
pub trait KeyExt {
    /// Converts into a [`CounterKey`].
    fn into_counter(self) -> CounterKey;
    /// Converts into a [`GaugeKey`].
    fn into_gauge(self) -> GaugeKey;
    /// Converts into a [`HistogramKey`].
    fn into_histogram(self) -> HistogramKey;
}

impl KeyExt for Key {
    fn into_counter(self) -> CounterKey {
        CounterKey(self)
    }

    fn into_gauge(self) -> GaugeKey {
        GaugeKey(self)
    }

    fn into_histogram(self) -> HistogramKey {
        HistogramKey(self)
    }
}

impl KeyExt for &'static str {
    fn into_counter(self) -> CounterKey {
        Key::from_static_name(self).into_counter()
    }

    fn into_gauge(self) -> GaugeKey {
        Key::from_static_name(self).into_gauge()
    }

    fn into_histogram(self) -> HistogramKey {
        Key::from_static_name(self).into_histogram()
    }
}

/// Read-only view over metric values.
pub trait MetricsRead {
    /// Returns a counter value for `key`.
    fn get_counter(&self, key: &Key) -> Option<CounterValue>;
    /// Returns a gauge value for `key`.
    fn get_gauge(&self, key: &Key) -> Option<GaugeValue>;
    /// Returns histogram values for `key`.
    fn get_histogram(&self, key: &Key) -> Option<HistogramValue>;

    /// Reads a typed metric value with a typed key.
    fn get<K: ReadKey>(&self, key: &K) -> Option<K::Value> {
        key.read_from(self)
    }
}

/// Typed key abstraction over a concrete metric value type.
pub trait ReadKey {
    /// Value produced by this key.
    type Value: PartialEq;

    /// Reads this key from a [`MetricsRead`] implementation.
    fn read_from<R: MetricsRead + ?Sized>(
        &self,
        reader: &R,
    ) -> Option<Self::Value>;
}

impl ReadKey for CounterKey {
    type Value = CounterValue;

    fn read_from<R: MetricsRead + ?Sized>(
        &self,
        reader: &R,
    ) -> Option<Self::Value> {
        reader.get_counter(self.as_key())
    }
}

impl ReadKey for GaugeKey {
    type Value = GaugeValue;

    fn read_from<R: MetricsRead + ?Sized>(
        &self,
        reader: &R,
    ) -> Option<Self::Value> {
        reader.get_gauge(self.as_key())
    }
}

impl ReadKey for HistogramKey {
    type Value = HistogramValue;

    fn read_from<R: MetricsRead + ?Sized>(
        &self,
        reader: &R,
    ) -> Option<Self::Value> {
        reader.get_histogram(self.as_key())
    }
}
