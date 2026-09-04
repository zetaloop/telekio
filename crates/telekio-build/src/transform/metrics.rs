use std::{error::Error, path::Path};

use super::{mount, patch};
use crate::edit;

pub(super) fn patch_histogram(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        edit::add_attr(
            source,
            edit::AttrTarget::Method {
                owner: "HistogramType",
                name: "bucket_range",
            },
            "#[cfg_attr(all(not(test), target_has_atomic = \"64\"), expect(dead_code))]",
        )?;
        edit::add_attr(
            source,
            edit::AttrTarget::Impl {
                owner: "Histogram",
                method: "num_buckets",
            },
            "#[cfg_attr(target_has_atomic = \"64\", expect(dead_code))]",
        )?;
        mount(
            source,
            None,
            "telekio",
            "guest/runtime/metrics/histogram.rs",
        )?;
        edit::add_attr(
            source,
            edit::AttrTarget::Module("telekio"),
            "#[cfg(tokio_unstable)]",
        )
    })
}

pub(super) fn patch_worker_metrics(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        edit::add_attr(
            source,
            edit::AttrTarget::Method {
                owner: "WorkerMetrics",
                name: "queue_depth",
            },
            "#[expect(dead_code)]",
        )?;
        edit::add_attr(
            source,
            edit::AttrTarget::Method {
                owner: "WorkerMetrics",
                name: "thread_id",
            },
            "#[cfg_attr(target_has_atomic = \"64\", expect(dead_code))]",
        )
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
            edit::redirect_call(
                source,
                edit::Scope::Method {
                    owner: "RuntimeMetrics",
                    name,
                },
                call,
                replacement,
            )?;
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
            edit::redirect_call(
                source,
                edit::Scope::Method {
                    owner: "RuntimeMetrics",
                    name,
                },
                "worker_metrics",
                "host_worker_metrics",
            )?;
        }
        edit::redirect_call(
            source,
            edit::Scope::Method {
                owner: "RuntimeMetrics",
                name: "spawned_tasks_count",
            },
            "spawned_tasks_count",
            "host_spawned_tasks_count",
        )?;
        for name in ["remote_schedule_count", "budget_forced_yield_count"] {
            edit::redirect_call(
                source,
                edit::Scope::Method {
                    owner: "RuntimeMetrics",
                    name,
                },
                "scheduler_metrics",
                "host_scheduler_metrics",
            )?;
        }
        edit::add_attr(
            source,
            edit::AttrTarget::Method {
                owner: "RuntimeMetrics",
                name: "with_io_driver_metrics",
            },
            "#[expect(dead_code)]",
        )?;
        for (name, replacement) in [
            ("io_driver_fd_registered_count", "host_io_driver_registered"),
            (
                "io_driver_fd_deregistered_count",
                "host_io_driver_deregistered",
            ),
            ("io_driver_ready_count", "host_io_driver_ready"),
        ] {
            edit::redirect_call(
                source,
                edit::Scope::Method {
                    owner: "RuntimeMetrics",
                    name,
                },
                "with_io_driver_metrics",
                replacement,
            )?;
        }
        mount(source, None, "telekio", "guest/runtime/metrics/runtime.rs")
    })
}
