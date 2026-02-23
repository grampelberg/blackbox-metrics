# blackbox-metrics

A local, in-process metrics recorder for debugging, reporting and testing.

Metrics are useful for much more than feeding a centralized observability
stack.

Trying to track down a race condition and wish you could just dump metric state
locally? `BlackboxRecorder` gives you that immediately.

Want to show users useful live numbers without running Prometheus, Grafana, or
another external service? `Snapshot` and `Sampler` are built for that.

Tired of plumbing internal state through tests when the behavior is already
visible through metrics? Assert directly on emitted metrics instead.

## Debugging

Add a tool beyond `println!` statements to your toolbox. Metrics can show you
how many times something is happening (maybe even none) and track basic state
transitions that are tough to reason about in log lines (is a connection still
active?). Instrument your code the way you would normally with the metrics
macros, such as `metrics::counter` and then use the recorder to get a snapshot.

Perhaps the easiest way to go about this would be by dumping the entire state to
terminal on drop.

```rust
use blackbox_metrics::BlackboxRecorder;
use metrics::{counter, with_local_recorder};

let recorder = BlackboxRecorder::default();

let guard = recorder.dump();

with_local_recorder(&recorder, || {
    counter!("requests_total").increment(3);
});

drop(guard);
```

Note: you can also capture and print snapshots normally via
`recorder.snapshot()`. Imagine adding this to a loop to understand what's going
on each time it executes.

## Reporting

You can expose live state of metrics and report on them. Imagine being able to
show how many times a clap command was run or provide the rate that requests are
coming in. The `Sampler` will periodically sample metrics and provide the series
to you for reporting.

```rust
use std::time::Duration;

use blackbox_metrics::{BlackboxRecorder, KeyExt};
use metrics::{counter, with_local_recorder};

let runtime = tokio::runtime::Runtime::new().unwrap();
runtime.block_on(async {
    let recorder = BlackboxRecorder::default();
    let key = "requests_total".into_counter();

    let (handle, run) = recorder.sampler(vec![key.clone()]).into_runner();
    let worker = tokio::spawn(run);

    with_local_recorder(&recorder, || {
        counter!("requests_total").increment(1);
    });
    tokio::time::sleep(Duration::from_millis(1_100)).await;

    if let Some(series) = handle.series(&key) {
        println!("total={} rate={:?}/s", series.stats.total, series.stats.rate);
    }

    worker.abort();
});
```

## Testing

There's no need to plumb state through your public API to test expectations.
Writing integrations tests can assert that something specific happened and take
care of two things at once: metrics are consumable and reliable in addition to
specific behaviors that might be important.

```rust
use blackbox_metrics::{BlackboxRecorder, KeyExt};
use metrics::{counter, with_local_recorder};

#[tokio::test]
async fn increments_requests_counter() {
    let recorder = BlackboxRecorder::default();

    with_local_recorder(&recorder, || {
        counter!("requests_total").increment(2);
    });

    recorder
        .assert(&"requests_total".into_counter(), 2)
        .await
        .unwrap();
}
```
