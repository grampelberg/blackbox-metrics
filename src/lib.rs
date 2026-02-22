mod dump_guard;
mod recorder;
pub mod sampler;
mod snapshot;

use metrics::Key;

pub use crate::recorder::BlackboxRecorder;

type BoxError = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("deadline exceeded")]
    Deadline,
    #[error("metrics: {0}")]
    Metrics(BoxError),
    #[error("key has not been registered")]
    NoKey,
}

pub type CounterValue = u64;
pub type GaugeValue = f64;
pub type HistogramValue = Vec<f64>;

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct CounterKey(Key);

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct GaugeKey(Key);

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct HistogramKey(Key);

impl CounterKey {
    pub fn as_key(&self) -> &Key {
        &self.0
    }
}

impl GaugeKey {
    pub fn as_key(&self) -> &Key {
        &self.0
    }
}

impl HistogramKey {
    pub fn as_key(&self) -> &Key {
        &self.0
    }
}

pub trait KeyExt {
    fn into_counter(self) -> CounterKey;
    fn into_gauge(self) -> GaugeKey;
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

pub trait MetricsRead {
    fn get_counter(&self, key: &Key) -> Option<CounterValue>;
    fn get_gauge(&self, key: &Key) -> Option<GaugeValue>;
    fn get_histogram(&self, key: &Key) -> Option<HistogramValue>;

    fn get<K: ReadKey>(&self, key: &K) -> Option<K::Value> {
        key.read_from(self)
    }
}

pub trait ReadKey {
    type Value: PartialEq;

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
