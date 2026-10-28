use super::*;
use crate::ProcessInfo;
use std::{
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    time::Duration,
};

struct OwnedChild {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    file: PathBuf,
    cpu_seconds: f64,
}

impl OwnedChild {
    fn new() -> Self {
        let script = r#"import os, sys, time
f = open(sys.argv[1] + str(os.getpid()), 'wb', buffering=0)
print('ready', time.process_time(), flush=True)
for line in sys.stdin:
    size, busy = map(int, line.split())
    f.write(b'x' * size)
    os.fsync(f.fileno())
    end = time.process_time() + busy / 1000
    while time.process_time() < end:
        pass
    print('done', time.process_time(), flush=True)
"#;
        let prefix = std::env::temp_dir().join("gtop-transition-");
        let mut child = Command::new("/usr/bin/python3")
            .args(["-c", script, prefix.to_str().unwrap()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut ready = String::new();
        output.read_line(&mut ready).unwrap();
        let cpu_seconds = ready
            .trim()
            .strip_prefix("ready ")
            .unwrap()
            .parse()
            .unwrap();
        let file = PathBuf::from(format!("{}{}", prefix.display(), child.id()));
        Self {
            child,
            input,
            output,
            file,
            cpu_seconds,
        }
    }

    fn work(&mut self, bytes: usize, cpu_ms: usize) -> f64 {
        writeln!(self.input, "{bytes} {cpu_ms}").unwrap();
        self.input.flush().unwrap();
        let mut done = String::new();
        self.output.read_line(&mut done).unwrap();
        let cpu_seconds = done.trim().strip_prefix("done ").unwrap().parse().unwrap();
        cpu_seconds - std::mem::replace(&mut self.cpu_seconds, cpu_seconds)
    }

    fn interval(&mut self, bytes: usize, cpu_ms: usize) -> f64 {
        let start = Instant::now();
        let cpu_seconds = self.work(bytes, cpu_ms);
        std::thread::sleep(Duration::from_secs(1).saturating_sub(start.elapsed()));
        cpu_seconds
    }

    fn row(&self, state: &AppState, failed: bool) -> ProcessInfo {
        if failed {
            state
                .process_monitor
                .lock()
                .unwrap()
                .fail_next_native_collection();
        }
        let snapshot = state.snapshot(false, false).unwrap();
        assert_eq!(
            matches!(snapshot.process_data, ProcessData::Stale { .. }),
            failed
        );
        snapshot
            .processes
            .iter()
            .find(|row| row.pid == self.child.id())
            .unwrap()
            .clone()
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.file);
    }
}

fn assert_cpu_work(row: &ProcessInfo, expected_seconds: f64) {
    // Scheduling delays change the percentage, not the completed CPU work.
    let seconds = row.cpu_usage as f64 * row.cpu_sample_seconds.unwrap() / 100.;
    assert!(
        (seconds - expected_seconds).abs() < 0.02,
        "expected {expected_seconds}s CPU work, got {seconds}s from {}% over {:?}s",
        row.cpu_usage,
        row.cpu_sample_seconds
    );
}

#[test]
fn snapshots_keep_stale_rows_and_recover_exact_counter_intervals() {
    const BYTES: usize = 8 * 1024 * 1024;
    let mut child = OwnedChild::new();
    child.work(BYTES, 100);
    let state = AppState::new();
    let first_work = child.interval(BYTES, 200);
    let first = child.row(&state, false);
    assert_eq!(first.disk_usage.1, BYTES as u64);
    assert_cpu_work(&first, first_work);
    let stale_work = child.interval(BYTES, 200);
    let stale = child.row(&state, true);
    assert_eq!(stale.disk_usage, first.disk_usage);
    assert_eq!(stale.cpu_usage, first.cpu_usage);
    let recovery_work = child.interval(BYTES, 200);
    let recovered = child.row(&state, false);
    assert_eq!(recovered.disk_usage.1, 2 * BYTES as u64);
    assert_cpu_work(&recovered, stale_work + recovery_work);
    for failed in [true, true, false] {
        child.interval(0, 0);
        let row = child.row(&state, failed);
        if !failed {
            assert_eq!(row.disk_usage.1, 0);
            assert_cpu_work(&row, 0.);
        }
    }
    child.child.kill().unwrap();
    child.child.wait().unwrap();
    assert!(!state
        .snapshot(false, false)
        .unwrap()
        .processes
        .iter()
        .any(|p| p.pid == child.child.id()));
    assert!(state.sys.lock().unwrap().processes().is_empty());
}

#[test]
fn failed_startup_primes_first_success_and_preserves_new_process_writes() {
    const BYTES: usize = 8 * 1024 * 1024;
    let mut old = OwnedChild::new();
    old.work(BYTES, 0);
    let mut monitor = ProcessMonitor::new();
    monitor.fail_next_native_collection();
    let state = AppState::with_process_monitor(monitor);
    state
        .process_monitor
        .lock()
        .unwrap()
        .fail_next_native_collection();
    let failed = state.snapshot(false, false).unwrap();
    assert!(matches!(failed.process_data, ProcessData::Stale { .. }));
    assert!(failed.processes.is_empty());
    assert!(failed.stats.memory_total > 0);
    assert_eq!(old.row(&state, false).disk_usage.1, 0);
    let mut new = OwnedChild::new();
    new.work(BYTES, 0);
    assert_eq!(new.row(&state, false).disk_usage.1, BYTES as u64);
    assert_eq!(new.row(&state, false).disk_usage.1, 0);
}

#[test]
fn unavailable_counters_preserve_last_valid_samples_and_startup_baselines() {
    const BYTES: usize = 8 * 1024 * 1024;
    let mut child = OwnedChild::new();
    child.work(BYTES, 500);
    let mut monitor = ProcessMonitor::new();
    monitor.fail_next_counter_reads(child.child.id());
    let state = AppState::with_process_monitor(monitor);
    let first = child.row(&state, false);
    assert_eq!(first.disk_usage.1, 0);
    assert_eq!(first.cpu_usage, 0.);
    child.interval(0, 0);
    let last_valid_before = std::time::Instant::now();
    let idle = child.row(&state, false);
    let last_valid_after = std::time::Instant::now();
    assert_eq!(idle.disk_usage.1, 0);
    assert_cpu_work(&idle, 0.);
    let unavailable_work = child.interval(BYTES, 200);
    state
        .process_monitor
        .lock()
        .unwrap()
        .fail_next_counter_reads(child.child.id());
    let unavailable = child.row(&state, false);
    assert_eq!(unavailable.disk_usage, (0, 0));
    assert!(unavailable.disk_sample_seconds.is_none());
    assert!(unavailable.cpu_sample_seconds.is_none());
    assert_eq!(unavailable.cpu_usage, 0.);
    assert_eq!(unavailable.memory_usage, 0);
    let recovery_work = child.interval(BYTES, 0);
    let recovery_before = std::time::Instant::now();
    let recovery = child.row(&state, false);
    let recovery_after = std::time::Instant::now();
    assert_eq!(recovery.disk_usage.1, 2 * BYTES as u64);
    let span = recovery.disk_sample_seconds.unwrap();
    assert!(recovery.cpu_sample_seconds.unwrap() >= span * 0.95);
    let shortest = recovery_before
        .duration_since(last_valid_after)
        .as_secs_f64();
    let longest = recovery_after
        .duration_since(last_valid_before)
        .as_secs_f64();
    assert!((shortest..=longest).contains(&span));
    assert!(span > 1.9);
    assert_cpu_work(&recovery, unavailable_work + recovery_work);
    child.interval(0, 0);
    let idle = child.row(&state, false);
    assert_eq!(idle.disk_usage.1, 0);
    assert_cpu_work(&idle, 0.);
}
