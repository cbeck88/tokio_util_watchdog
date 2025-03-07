use std::time::Duration;
use tokio::runtime;

/// Data collected from tokio::runtime::RuntimeMetrics
///
/// https://docs.rs/tokio/latest/tokio/runtime/struct.RuntimeMetrics.html
///
/// Annoyingly, debug logging that thing doesn't do quite what you would expect.
///
/// I didn't want to use the tokio_metrics crate because that thing assumes that
/// you want to continuously publish these metrics, rather than just helping me
/// process snapshots from tokio.
///
/// To populate this, usually one might do like `TokioRuntimeMetrics::from(Handle::current())`
/// or simply `TokioRuntimeMetrics::current()`.
///
/// The only thing you can do with this is debug print it.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct TokioRuntimeMetrics {
    /// Number of worker threads
    num_workers: usize,
    /// Number of active tasks
    num_alive_tasks: usize,
    /// Global queue depth
    global_queue_depth: usize,
    /// Number of blocking threads
    #[cfg(tokio_unstable)]
    num_blocking_threads: usize,
    /// Number of idle blocking threads
    #[cfg(tokio_unstable)]
    num_idle_blocking_threads: usize,
    /// The number of tasks currently in the blocking tasks queue, created via spawn_blocking.
    #[cfg(tokio_unstable)]
    blocking_queue_depth: usize,
    /// Total number of tasks spawned on this runtime
    #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
    spawned_tasks_count: u64,
    /// Number of times that a thread outside the runtime has scheduled a task
    #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
    remote_schedule_count: u64,
    /// Number of times that tasks have been forced to yield back to the scheduler after exhausting their task budgets.
    #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
    budget_forced_yield_count: u64,
    /// How many times each worker has parked
    worker_park_count: Vec<u64>,
    /// How many times each worker has woken up and immediately parked again
    worker_noop_count: Vec<u64>,
    /// How many tasks each worker has stolen from another worker thread
    worker_steal_count: Vec<u64>,
    /// How many steal operations each worker has performed which stole at least one task
    worker_steal_operations: Vec<u64>,
    /// How many times each worker has polled a task
    worker_poll_count: Vec<u64>,
    /// The total amount of time each worker has been busy
    worker_total_busy_duration: Vec<Duration>,
    /// How many times a worker has scheduled a task (from within the runtime) onto its own queue
    worker_local_schedule_count: Vec<u64>,
    /// How many times a worker's local queue has become full. When this happens it sends tasks to the injection queue
    worker_overflow_count: Vec<u64>,
    /// The number of tasks currently in each worker's local queue
    worker_local_queue_depth: Vec<usize>,
    /// The mean duration of task poll times for each worker. This is an exponentially weighted moving average for each worker.
    worker_mean_poll_time: Vec<Duration>,
}

impl TokioRuntimeMetrics {
    /// Construct self from a tokio RuntimeMetrics object
    #[allow(unused_mut)]
    pub fn new(src: runtime::RuntimeMetrics) -> Self {
        let num_workers = src.num_workers();
        let mut worker_park_count = vec![];
        let mut worker_noop_count = vec![];
        let mut worker_steal_count = vec![];
        let mut worker_steal_operations = vec![];
        let mut worker_poll_count = vec![];
        let mut worker_total_busy_duration = vec![];
        let mut worker_local_schedule_count = vec![];
        let mut worker_overflow_count = vec![];
        let mut worker_local_queue_depth = vec![];
        let mut worker_mean_poll_time = vec![];

        #[cfg(tokio_unstable)]
        for i in 0..num_workers {
            worker_local_queue_depth.push(src.worker_local_queue_depth(i));
        }

        #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
        for i in 0..num_workers {
            worker_park_count.push(src.worker_park_count(i));
            worker_noop_count.push(src.worker_noop_count(i));
            worker_steal_count.push(src.worker_steal_count(i));
            worker_steal_operations.push(src.worker_steal_operations(i));
            worker_poll_count.push(src.worker_poll_count(i));
            worker_total_busy_duration.push(src.worker_total_busy_duration(i));
            worker_local_schedule_count.push(src.worker_local_schedule_count(i));
            worker_overflow_count.push(src.worker_overflow_count(i));
            worker_mean_poll_time.push(src.worker_mean_poll_time(i));
        }

        Self {
            num_workers,
            num_alive_tasks: src.num_alive_tasks(),
            global_queue_depth: src.global_queue_depth(),
            #[cfg(tokio_unstable)]
            blocking_queue_depth: src.blocking_queue_depth(),
            #[cfg(tokio_unstable)]
            num_blocking_threads: src.num_blocking_threads(),
            #[cfg(tokio_unstable)]
            num_idle_blocking_threads: src.num_idle_blocking_threads(),
            #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
            spawned_tasks_count: src.spawned_tasks_count(),
            #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
            remote_schedule_count: src.remote_schedule_count(),
            #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
            budget_forced_yield_count: src.budget_forced_yield_count(),
            worker_park_count,
            worker_noop_count,
            worker_steal_count,
            worker_steal_operations,
            worker_poll_count,
            worker_total_busy_duration,
            worker_local_schedule_count,
            worker_overflow_count,
            worker_local_queue_depth,
            worker_mean_poll_time,
        }
    }

    /// Construct self using metrics from the current tokio runtime.
    /// Panics if there is no current runtime.
    pub fn current() -> Self {
        Self::from(&runtime::Handle::current())
    }
}

impl From<&runtime::Handle> for TokioRuntimeMetrics {
    fn from(src: &runtime::Handle) -> Self {
        Self::new(src.metrics())
    }
}
