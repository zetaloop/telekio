use std::{error::Error, path::Path};

use r#override::{item, root};

use super::{mount, patch};

pub(super) fn patch_histogram(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        source
            .select(item("HistogramType::bucket_range"))?
            .add_attribute(
                "#[cfg_attr(all(not(test), target_has_atomic = \"64\"), expect(dead_code))]",
            )?;
        source
            .select(root().implementation("Histogram").has(item("num_buckets")))?
            .add_attribute("#[cfg_attr(target_has_atomic = \"64\", expect(dead_code))]")?;
        mount(
            source,
            None,
            "telekio",
            "guest/runtime/metrics/histogram.rs",
        )?;
        source
            .select(item("telekio"))?
            .add_attribute("#[cfg(tokio_unstable)]")
    })
}

pub(super) fn patch_worker_metrics(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        source
            .select(item("WorkerMetrics::queue_depth"))?
            .add_attribute("#[expect(dead_code)]")?;
        source
            .select(item("WorkerMetrics::thread_id"))?
            .add_attribute("#[cfg_attr(target_has_atomic = \"64\", expect(dead_code))]")
    })
}

pub(super) fn patch_runtime_metrics(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        for (name, call, replacement) in [
            ("num_workers", "num_workers", "host_num_workers"),
            ("num_alive_tasks", "num_alive_tasks", "host_num_alive_tasks"),
            (
                "global_queue_depth",
                "injection_queue_depth",
                "host_global_queue_depth",
            ),
            (
                "num_blocking_threads",
                "num_blocking_threads",
                "host_num_blocking_threads",
            ),
            (
                "num_idle_blocking_threads",
                "num_idle_blocking_threads",
                "host_num_idle_blocking_threads",
            ),
            (
                "injection_queue_depth",
                "injection_queue_depth",
                "host_global_queue_depth",
            ),
            (
                "worker_local_queue_depth",
                "worker_local_queue_depth",
                "host_worker_local_queue_depth",
            ),
            (
                "blocking_queue_depth",
                "blocking_queue_depth",
                "host_blocking_queue_depth",
            ),
        ] {
            source
                .select(
                    root()
                        .implementation("RuntimeMetrics")
                        .item(name)
                        .call(call),
                )?
                .redirect(replacement)?;
        }
        for name in [
            "worker_total_busy_duration",
            "worker_park_count",
            "worker_park_unpark_count",
            "worker_thread_id",
            "poll_time_histogram_enabled",
            "poll_time_histogram_num_buckets",
            "poll_time_histogram_bucket_range",
            "worker_noop_count",
            "worker_steal_count",
            "worker_steal_operations",
            "worker_poll_count",
            "worker_local_schedule_count",
            "worker_overflow_count",
            "poll_time_histogram_bucket_count",
            "worker_mean_poll_time",
            "schedule_latency_histogram_enabled",
            "schedule_latency_histogram_num_buckets",
            "schedule_latency_histogram_bucket_range",
            "schedule_latency_histogram_bucket_count",
        ] {
            source
                .select(
                    root()
                        .implementation("RuntimeMetrics")
                        .item(name)
                        .call("worker_metrics"),
                )?
                .redirect("host_worker_metrics")?;
        }
        source
            .select(item("RuntimeMetrics::spawned_tasks_count").call("spawned_tasks_count"))?
            .redirect("host_spawned_tasks_count")?;
        for name in ["remote_schedule_count", "budget_forced_yield_count"] {
            source
                .select(
                    root()
                        .implementation("RuntimeMetrics")
                        .item(name)
                        .call("scheduler_metrics"),
                )?
                .redirect("host_scheduler_metrics")?;
        }
        source
            .select(item("RuntimeMetrics::with_io_driver_metrics"))?
            .add_attribute("#[expect(dead_code)]")?;
        for (name, replacement) in [
            ("io_driver_fd_registered_count", "host_io_driver_registered"),
            (
                "io_driver_fd_deregistered_count",
                "host_io_driver_deregistered",
            ),
            ("io_driver_ready_count", "host_io_driver_ready"),
        ] {
            source
                .select(
                    root()
                        .implementation("RuntimeMetrics")
                        .item(name)
                        .call("with_io_driver_metrics"),
                )?
                .redirect(replacement)?;
        }
        mount(source, None, "telekio", "guest/runtime/metrics/runtime.rs")
    })
}
