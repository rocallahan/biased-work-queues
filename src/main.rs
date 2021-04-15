use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use structopt::StructOpt;

mod biased_work_queue;

use biased_work_queue::*;

const WORK_SIZE: u64 = 1_000_000;

/// Simulate one step of the first "serial producer" stage of the workload.
#[inline(never)]
fn work_stage_1(num_worker_threads: u64) {
    let atomic = AtomicU64::new(0);
    // Take long enough that half the worker threads, working together,
    // can just keep up with us.
    let count = 2 * WORK_SIZE / num_worker_threads;
    for _ in 0..count {
        atomic.fetch_add(1, Ordering::Relaxed);
    }
}

/// Simulate one step of the second "parallel consumer" stage of the workload.
#[inline(never)]
fn work_stage_2() {
    let atomic = AtomicU64::new(0);
    for _ in 0..WORK_SIZE {
        atomic.fetch_add(1, Ordering::Relaxed);
    }
}

/// Run test with naive crossbeam-channel work queue
fn do_crossbeam(num_worker_threads: u64, total_work_units: u64, batch_size: u64) {
    let (work_tx, work_rx) = crossbeam::channel::unbounded();
    let work_rx_ref = &work_rx;
    crossbeam::scope(move |s| {
        for i in 0..num_worker_threads {
            s.builder()
                .name(format!("crossbeam-{}", i))
                .spawn(move |_| {
                    while let Ok(()) = work_rx_ref.recv() {
                        for _ in 0..batch_size {
                            work_stage_2();
                        }
                    }
                })
                .unwrap();
        }
        for _ in 0..(total_work_units / batch_size) {
            for _ in 0..batch_size {
                work_stage_1(num_worker_threads);
            }
            work_tx.send(()).unwrap();
        }
    })
    .unwrap();
}

/// Run test with a biased work queue based on crossbeam-channel
fn do_crossbeam_biased(num_worker_threads: u64, total_work_units: u64, _batch_size: u64) {
    let (work_tx, work_rx) = crossbeam::channel::unbounded();
    let work_rxs = biased_work_queue(work_rx, num_worker_threads as usize);
    crossbeam::scope(move |s| {
        for (i, work_rx) in work_rxs.into_iter().enumerate() {
            s.builder()
                .name(format!("biased-{}", i))
                .spawn(move |_| {
                    while let Ok(items) = work_rx.recv() {
                        for _ in items {
                            work_stage_2();
                        }
                    }
                })
                .unwrap();
        }
        for _ in 0..total_work_units {
            work_stage_1(num_worker_threads);
            work_tx.send(()).unwrap();
        }
    })
    .unwrap();
}

/// Run test with Rayon
fn do_rayon(num_worker_threads: u64, total_work_units: u64, batch_size: u64) {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(num_worker_threads as usize)
        .thread_name(|i| format!("rayon-{}", i))
        .build()
        .unwrap();
    for _ in 0..(total_work_units / batch_size) {
        for _ in 0..batch_size {
            work_stage_1(num_worker_threads);
        }
        pool.spawn(move || {
            for _ in 0..batch_size {
                work_stage_2()
            }
        });
    }
}

#[derive(StructOpt)]
enum Subcommand {
    #[structopt(name = "crossbeam")]
    Crossbeam,
    #[structopt(name = "rayon")]
    Rayon,
    #[structopt(name = "crossbeam-biased")]
    CrossbeamBiased,
}

#[derive(StructOpt)]
struct Opt {
    #[structopt(long = "batch-size")]
    batch_size: Option<u64>,
    #[structopt(subcommand)]
    subcommand: Subcommand,
}

fn main() {
    let opt = Opt::from_args();
    let threads = num_cpus::get() as u64;
    /// Make sure we do plenty of work per thread
    let total_work_units_per_pool = threads * 50;
    /// Each threadpool uses a bit over half of the available CPU power
    /// if run alone.
    /// We want to saturate the system, so run three instances at once.
    let simultaneous_pools = 3;
    let op = match opt.subcommand {
        Subcommand::Crossbeam => do_crossbeam,
        Subcommand::Rayon => do_rayon,
        Subcommand::CrossbeamBiased => do_crossbeam_biased,
    };
    let batch_size = opt.batch_size.unwrap_or(1);
    let mut handles = Vec::new();
    for _ in 0..simultaneous_pools {
        handles.push(thread::spawn(move || {
            op(threads, total_work_units_per_pool, batch_size)
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
}
