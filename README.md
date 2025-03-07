# tokio_util_watchdog

If we get a tokio deadlock, i.e. all worker threads get blocked and no more
asynchronous futures can be driven, it can be hard to diagnose and debug
in production.

This watchdog uses a very simple strategy to detect and try to recover from that
situation:

* Spawn a task on the runtime that periodically records "heartbeats", e.g. once a second.
* Spawn a thread (using std) outside of the runtime that wakes up periodically and checks
  for those heartbeats.
* If heartbeats are not detected for a few seconds (configurable), panic.
* Before we panic, try to collect and log [`tokio::RuntimeMetrics`](https://docs.rs/tokio/latest/tokio/runtime/struct.RuntimeMetrics.html) for this runtime for a few seconds (configurable).
* If `cfg(tokio_unstable)` and `cfg(tokio_taskdump)` were used, also try to collect and log a task dump for a few seconds.

The assumption here is that when the panic occurs, your deployment infrastructure will detect that this happened
and restart the process. Hopefully the process will recover and not immediately deadlock again. And meanwhile, you
will automatically get more information than you would otherwise, which might help you fix the underlying issue,
especially if you used the extra features.

(If you used django in the past, you might have seen similar behavior, where timed-out worker processes are automatically
killed and restarted, with some error logging, without blocking or starving the whole webserver.)

Note that this is a different type of watchdog from e.g. [`simple-tokio-watchdog`](https://crates.io/crates/simple-tokio-watchdog) and
some other such crates -- our crate is specifically for checking the tokio runtime itself for liveness, and then logging any useful diagnostics
and panicking (configurable).

## Quick start

1. Add `tokio_util_watchdog = "0.1"` to your `Cargo.toml`.
1. In `main.rs` somewhere, add lines such as:

```
use tokio_util_watchdog::Watchdog;

...

#[tokio::main]
async fn main() {
    ...

    let _watchdog = Watchdog::builder().build();

    ...
}
```

See the builder documentation for configuration options. The watchdog is disarmed gracefully if it is dropped.

**Optional:**

In `.cargo/config.toml`, add content such as:

```
# We only enable tokio_taskdump on Linux targets since it's not supported on Mac
[build]
rustflags = ["--cfg", "tokio_unstable"]

[target.x86_64-unknown-linux-gnu]
rustflags = ["--cfg", "tokio_unstable", "--cfg", "tokio_taskdump"]

[target.aarch64-unknown-linux-gnu]
rustflags = ["--cfg", "tokio_unstable", "--cfg", "tokio_taskdump"]
```

This will enable collection of additional [`tokio::RuntimeMetrics`](https://docs.rs/tokio/latest/tokio/runtime/struct.RuntimeMetrics.html)
and task dumps, which will be logged if a deadlock is detected.

Note: Since some parts of `tokio::RuntimeMetrics` were stabilized, you can still get some data without this, although you will miss many metrics
and won't get task dumps. See [tokio unstable features documentation](https://docs.rs/tokio/latest/tokio/index.html#unstable-features).

## Pros and Cons

Some types of deployment infrastructure will do external liveness checking of your process, e.g. using http requests.
Then, if this check fails, your process might get SIGTERM before SIGKILL, so you could try to tie this type of data collection and logging to
the SIGTERM signal handler instead of an internal timer.

There are a few advantages that I've seen to the internal watchdog timer approach:

* Not everything that uses async rust is an http server, and adding an http server just for liveness checks may feel heavy or awkward, as you will also have to configure it.
* Signal handling can itself be a can of worms.
* Sometimes if there are deadlocks in your system, a good way to reproduce them is to set `TOKIO_NUM_WORKERS` to 2 or 1, and
  exercise some part of your system via integration tests in CI. You may want those tests to be very simple and not involve docker etc.,
  and at that point internal liveness checking such as by this watchdog may be attractive.
* The in-process heartbeat system is really very simple, whereas with http-based liveness checking, it could be failing because of a networking issue instead.
  Note that nothing stops you from using both and putting a longer timeout on the http-based check.
* I have not experienced any false positives from this system in production or in CI testing -- the watchdog triggering has always been traced back to an actual problem.

You do pay the cost of having an extra thread in your process, but it only wakes up once a second (configurable) and this is typically negligible.
Anyways, any scheme of getting more tokio metrics after your runtime is deadlocked will require you to have a thread somewhere outside the runtime that can still do some work.

Another option is to use the [`tokio_metrics`](https://github.com/tokio-rs/tokio-metrics) crate, which is geared towards always collecting these metrics and publishing them e.g. via prometheus. If you do that, you might choose to set `triggered_metrics_collections` to `0` on the watchdog, so that it won't bother collecting any metrics. You can still benefit from logging of task dumps performed by the watchdog, and you can even set `panic` to `false`, so that the only thing the watchdog does is attempt to collect task dumps and log them when heartbeats are missed.

## License

MIT
