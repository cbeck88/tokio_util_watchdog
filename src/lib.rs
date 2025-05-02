//! This crate provides utilities for trying to catch and debug a blocked tokio runtime.
//!
//! * A watchdog which consists of a thread outside of tokio, and also a task within tokio,
//!   which sends heartbeats to the watchdog thread. If the heartbeats do not come frequently
//!   enough, the watchdog decides that the tokio executor is probably blocked.
//!   It will then attempt to collect metrics from the runtime and log them,
//!   any other relevant info (backtraces would be ideal),
//!   and eventually panic, although this behavior is configurable by env.
//! * Helper functions for obtaining runtime metrics etc. are also exposed.

#![deny(missing_docs)]
#![allow(deprecated)]

use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::runtime;
#[allow(unused)]
use tracing::{error, info, warn, warn_span, Instrument};

mod tokio_runtime_metrics;

pub use tokio_runtime_metrics::TokioRuntimeMetrics;

#[derive(Clone, Debug)]
struct Config {
    heartbeat_period: Duration,
    watchdog_timeout: Duration,
    triggered_metrics_duration: Duration,
    triggered_metrics_collections: u32,
    task_dump_deadline: Duration,
    panic: bool,
    thread_name: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            heartbeat_period: Duration::from_secs(1),
            watchdog_timeout: Duration::from_secs(5),
            triggered_metrics_duration: Duration::from_secs(2),
            triggered_metrics_collections: 20,
            task_dump_deadline: Duration::from_secs(5),
            panic: true,
            thread_name: "tokio-watchdog".into(),
        }
    }
}

/// Builder which can configure a watchdog object.
#[derive(Clone, Debug, Default)]
pub struct Builder {
    config: Config,
}

impl Builder {
    /// Set the heartbeat period, i.e. how frequently the heartbeat task beats
    /// Defaults to 1s.
    pub fn heartbeat_period(mut self, d: Duration) -> Self {
        self.config.heartbeat_period = d;
        self
    }

    /// Set the watchdog timeout, i.e. how long we can go without seeing a heartbeat before the watchdog is triggered
    /// Defaults to 5s.
    pub fn watchdog_timeout(mut self, d: Duration) -> Self {
        self.config.watchdog_timeout = d;
        self
    }

    /// Set how long we collect metrics for when triggered.
    /// Defaults to 2s.
    pub fn triggered_metrics_duration(mut self, d: Duration) -> Self {
        self.config.triggered_metrics_duration = d;
        self
    }

    /// Set how many times we will try to collect metrics during the period.
    /// Defaults to 20. Set to 0 to disable metrics collection.
    pub fn triggered_metrics_collections(mut self, n: u32) -> Self {
        self.config.triggered_metrics_collections = n;
        self
    }

    /// Set how long we will wait for a taskdump when triggered.
    /// Defaults to 5s.
    pub fn task_dump_deadline(mut self, d: Duration) -> Self {
        self.config.task_dump_deadline = d;
        self
    }

    /// Set whether or not to panic when triggered. Defaults to true.
    pub fn panic(mut self, b: bool) -> Self {
        self.config.panic = b;
        self
    }

    /// Set the thread name. Defaults to "tokio-watchdog".
    pub fn thread_name(mut self, s: &str) -> Self {
        self.config.thread_name = s.to_owned();
        self
    }

    /// Build a watchdog instance for the current tokio runtime.
    /// Panics if there is no current runtime.
    pub fn build(self) -> Watchdog {
        self.build_for_runtime(runtime::Handle::current())
    }

    /// Build a watchdog instance for given tokio runtime
    pub fn build_for_runtime(self, handle: runtime::Handle) -> Watchdog {
        Watchdog::new_for_runtime(self.config, handle)
    }
}

/// The watchdog object monitors a given tokio runtime to see if it looks deadlocked.
///
/// It spawns a thread outside of the runtime which watches for heartbeats from an async task
/// that it spawns in the runtime.
///
/// When a long enough time passes without a heartbeat, the watchdog is "triggered".
///
/// By default, when it is triggered it will:
/// * Try to collect tokio runtime metrics for a few seconds and log them
/// * Try to log a tokio task dump for a few seconds (giving up if it doesn't succeed).
/// * Panic, so that the process can restart and hopefully recover.
///
/// The panic / restart idea is similar in spirit to how gunicorn / django will try to
/// restart worker processes that time out.
///
/// Dropping the watchdog will join the watchdog thread and the heartbeat task.
pub struct Watchdog {
    watchdog_thread: Option<std::thread::JoinHandle<()>>,
    stop_requested: Arc<AtomicBool>,
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        if let Some(handle) = self.watchdog_thread.take() {
            self.stop_requested.store(true, Ordering::SeqCst);
            handle.join().expect("Could not join watchdog thread");
        }
    }
}

impl Watchdog {
    /// Create a new watchdog builder, to configure a watchdog instance.
    pub fn builder() -> Builder {
        Builder::default()
    }

    /// Make a new watchdog for a given tokio runtime.
    /// Starts the heartbeat task and the watchdog thread.
    /// Drop this handle in order to stop both.
    fn new_for_runtime(config: Config, handle: runtime::Handle) -> Self {
        let stop_requested = Arc::new(AtomicBool::default());
        let thread_stop_requested = stop_requested.clone();

        let watchdog_thread = Some(
            std::thread::Builder::new()
                .name(format!("{}-thread", config.thread_name))
                .spawn(move || {
                    Self::watchdog_thread_entrypoint(config, handle, thread_stop_requested)
                })
                .expect("could not spawn thread"),
        );

        Self {
            watchdog_thread,
            stop_requested,
        }
    }

    fn exe_name() -> Option<String> {
        Some(
            Path::new(&std::env::args_os().next()?)
                .file_name()?
                .to_string_lossy()
                .as_ref()
                .to_owned(),
        )
    }

    fn watchdog_thread_entrypoint(
        config: Config,
        handle: runtime::Handle,
        stop_requested: Arc<AtomicBool>,
    ) {
        let exe_name = Self::exe_name().unwrap_or_else(|| "?".into());
        let span = warn_span!("watchdog", exe = exe_name);
        let span_clone = span.clone();
        span.in_scope(move || {
            #[allow(unused)]
            let Config {
                heartbeat_period,
                watchdog_timeout,
                triggered_metrics_duration,
                triggered_metrics_collections,
                task_dump_deadline,
                panic,
                thread_name,
            } = config;

            // The heartbeat channel is an std::Mutex < Instant > shared between the heartbeat task and the watchdog thread.
            // The heartbeat task updates it periodically, and the watchdog reads it periodically.
            let heartbeat_channel = Arc::new(Mutex::new(Instant::now()));
            let task_heartbeat_channel = heartbeat_channel.clone();
            let task_stop_requested = stop_requested.clone();
            let task_name = thread_name.clone();

            // Spawn the tokio task that will periodically update the heartbeat channel
            handle.spawn(async move {
                info!("{task_name} heartbeat task started");
                loop {
                    *task_heartbeat_channel.lock().unwrap() = Instant::now();
                    tokio::time::sleep(heartbeat_period).await;
                    if task_stop_requested.load(Ordering::SeqCst) {
                        info!("{task_name} heartbeat task stop requested");
                        break;
                    }
                }
            }.instrument(span_clone));

            info!("{thread_name} thread started");

            // Now enter the watchdog loop
            loop {
                let last_heartbeat = *heartbeat_channel.lock().unwrap();
                let elapsed = last_heartbeat.elapsed();

                if elapsed > watchdog_timeout {
                    error!("{thread_name} thread: Watchdog has been triggered: {elapsed:?} since last heartbeat > {watchdog_timeout:?}");

                    for i in 0..triggered_metrics_collections {
                        let metrics = TokioRuntimeMetrics::from(&handle);
                        warn!("Runtime metrics {i}/{triggered_metrics_collections}: {metrics:#?}");
                        std::thread::sleep(triggered_metrics_duration / triggered_metrics_collections);
                    }

                    // If task dumps are enabled, try also to acquire a task dump
                    // According to docu, a taskdump requires all polled futures to eventually yield,
                    // then it polls them in a special tracing mode. So this won't work if the runtime is actually
                    // deadlocked. If it's just slow however, then we will get this additional data.
                    // If the task dump works, then we cancel the panic.
                    // We have to run the task dump on a separate thread since the runtime may be FUBAR.
                    #[cfg(all(tokio_unstable, tokio_taskdump))]
                    {
                        use std::sync::Condvar;

                        warn!("{thread_name}: Attempting to collect a taskdump");

                        let pair = Arc::new((Mutex::new(false), Condvar::new()));
                        let thread_pair = pair.clone();
                        let thread_handle = handle.clone();

                        if let Err(err) = std::thread::Builder::new()
                            .name(format!("{thread_name}-taskdump"))
                            .spawn(move || {
                                let fut = tokio::time::timeout(task_dump_deadline, thread_handle.dump());
                                match thread_handle.block_on(fut) {
                                    Ok(dump) => {
                                        for (i, task) in dump.tasks().iter().enumerate() {
                                            let trace = task.trace().to_string();
                                            warn!(task = i, "{trace}");
                                        }
                                        let (lk, cvar) = &*thread_pair;
                                        *lk.lock().unwrap() = true;
                                        cvar.notify_one();
                                    }
                                    Err(err) => {
                                        warn!("task dump error: {err}");
                                    }
                                }
                            }) {
                            error!("{thread_name}: Could not spawn taskdump thread: {err}");
                        } else {
                            // Wait at least task_dump_deadline for the task dump job to complete.
                            let (lk, cvar) = &*pair;
                            let (gd, _timeout_result) = cvar.wait_timeout_while(
                                lk.lock().unwrap(),
                                task_dump_deadline,
                                |&mut done| !done
                            ).expect("Error waiting for condvar");

                            // Check if success was recorded
                            if *gd {
                                info!("{thread_name}: Task dump was succesful, this indicates that the runtime is not deadlocked. Watchdog is being reset");

                                // Re-enter the loop. We should not immediately retrigger watchdog, because if all futures polled successfully,
                                // then the heartbeat task should have run at least once in the last task_dump_deadline, which presumably is <= watchdog_timeout.
                                continue;
                            } else {
                                warn!("{thread_name}: Task dump was unsuccessful after {task_dump_deadline:?}");
                            }
                        }
                    }

                    if panic {
                        // Check if stop was requested immediately before panicking
                        if stop_requested.load(Ordering::SeqCst) {
                            info!("{thread_name} stop requested");
                            break;
                        }
                        panic!("{thread_name} panicked: {elapsed:?} since last heartbeat > {watchdog_timeout:?}, exe = {exe_name}");
                    }
                }

                // Sleep for a bit
                std::thread::sleep(heartbeat_period);

                // Exit if requested
                if stop_requested.load(Ordering::SeqCst) {
                    info!("{thread_name} stop requested");
                    break;
                }
            }
        })
    }
}
