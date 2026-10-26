use rusqlite::{params, Connection, TransactionBehavior};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

const RETENTION_SECONDS: u64 = 30 * 24 * 60 * 60;
pub const WINDOW_STATE_COUNT: usize = 5;

#[cfg(target_os = "macos")]
#[repr(C)]
struct Timebase {
    numer: u32,
    denom: u32,
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn mach_timebase_info(info: *mut Timebase) -> i32;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
pub enum WindowState {
    Visible = 0,
    Occluded = 1,
    Hidden = 2,
    Minimized = 3,
    Unknown = 4,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MinuteSample {
    pub wall_seconds: u64,
    pub process_cpu_seconds: f64,
    pub state_seconds: [f64; WINDOW_STATE_COUNT],
    pub state_cpu_seconds: [f64; WINDOW_STATE_COUNT],
    pub draw_count: u64,
    pub draw_total_ms: f64,
    pub sampler_runs: u64,
    pub sampler_total_ms: f64,
    pub history_write_ms: f64,
    pub refresh_interval_seconds: u64,
    pub app_version: &'static str,
    pub app_commit: &'static str,
}

pub struct PerfCounters {
    state_ns: [AtomicU64; WINDOW_STATE_COUNT],
    state_cpu_ns: [AtomicU64; WINDOW_STATE_COUNT],
    draw_count: AtomicU64,
    draw_total_ns: AtomicU64,
    sampler_runs: AtomicU64,
    sampler_total_ns: AtomicU64,
    history_write_ns: AtomicU64,
    refresh_interval_seconds: AtomicU64,
}

impl Default for PerfCounters {
    fn default() -> Self {
        Self {
            state_ns: std::array::from_fn(|_| AtomicU64::new(0)),
            state_cpu_ns: std::array::from_fn(|_| AtomicU64::new(0)),
            draw_count: AtomicU64::new(0),
            draw_total_ns: AtomicU64::new(0),
            sampler_runs: AtomicU64::new(0),
            sampler_total_ns: AtomicU64::new(0),
            history_write_ns: AtomicU64::new(0),
            refresh_interval_seconds: AtomicU64::new(3),
        }
    }
}

impl PerfCounters {
    pub fn add_window_time(&self, state: WindowState, elapsed: Duration) {
        self.state_ns[state as usize].fetch_add(
            elapsed.as_nanos().min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
    }

    pub fn add_process_cpu(&self, state: WindowState, elapsed: Duration) {
        self.state_cpu_ns[state as usize].fetch_add(
            elapsed.as_nanos().min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
    }

    pub fn add_draws(&self, count: u64, total: Duration) {
        self.draw_count.fetch_add(count, Ordering::Relaxed);
        self.draw_total_ns.fetch_add(
            total.as_nanos().min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
    }

    pub fn add_sampler(&self, elapsed: Duration) {
        self.sampler_runs.fetch_add(1, Ordering::Relaxed);
        self.sampler_total_ns.fetch_add(
            elapsed.as_nanos().min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
    }

    pub fn add_history_write(&self, elapsed: Duration) {
        self.history_write_ns.fetch_add(
            elapsed.as_nanos().min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
    }

    pub fn set_refresh_interval(&self, seconds: u64) {
        self.refresh_interval_seconds
            .store(seconds, Ordering::Relaxed);
    }

    pub fn take(&self, wall_seconds: u64) -> MinuteSample {
        let state_seconds = std::array::from_fn(|index| {
            self.state_ns[index].swap(0, Ordering::Relaxed) as f64 / 1_000_000_000.
        });
        let state_cpu_seconds = std::array::from_fn(|index| {
            self.state_cpu_ns[index].swap(0, Ordering::Relaxed) as f64 / 1_000_000_000.
        });
        let process_cpu_seconds = state_cpu_seconds.iter().sum();
        MinuteSample {
            wall_seconds,
            process_cpu_seconds,
            state_seconds,
            state_cpu_seconds,
            draw_count: self.draw_count.swap(0, Ordering::Relaxed),
            draw_total_ms: self.draw_total_ns.swap(0, Ordering::Relaxed) as f64 / 1_000_000.,
            sampler_runs: self.sampler_runs.swap(0, Ordering::Relaxed),
            sampler_total_ms: self.sampler_total_ns.swap(0, Ordering::Relaxed) as f64 / 1_000_000.,
            history_write_ms: self.history_write_ns.swap(0, Ordering::Relaxed) as f64 / 1_000_000.,
            refresh_interval_seconds: self.refresh_interval_seconds.load(Ordering::Relaxed),
            app_version: env!("CARGO_PKG_VERSION"),
            app_commit: option_env!("GTOP_GIT_COMMIT").unwrap_or("unknown"),
        }
    }

    fn restore(&self, sample: &MinuteSample) {
        for index in 0..WINDOW_STATE_COUNT {
            self.state_ns[index].fetch_add(
                (sample.state_seconds[index] * 1_000_000_000.) as u64,
                Ordering::Relaxed,
            );
            self.state_cpu_ns[index].fetch_add(
                (sample.state_cpu_seconds[index] * 1_000_000_000.) as u64,
                Ordering::Relaxed,
            );
        }
        self.draw_count
            .fetch_add(sample.draw_count, Ordering::Relaxed);
        self.draw_total_ns.fetch_add(
            (sample.draw_total_ms * 1_000_000.) as u64,
            Ordering::Relaxed,
        );
        self.sampler_runs
            .fetch_add(sample.sampler_runs, Ordering::Relaxed);
        self.sampler_total_ns.fetch_add(
            (sample.sampler_total_ms * 1_000_000.) as u64,
            Ordering::Relaxed,
        );
        self.history_write_ns.fetch_add(
            (sample.history_write_ms * 1_000_000.) as u64,
            Ordering::Relaxed,
        );
    }
}

pub struct PerfStore {
    connection: Connection,
}

pub struct PerfRecorder {
    counters: Arc<PerfCounters>,
    store: PerfStore,
    last_write: Duration,
}

impl PerfRecorder {
    pub fn open(path: &Path, counters: Arc<PerfCounters>) -> rusqlite::Result<Self> {
        Ok(Self {
            counters,
            store: PerfStore::open(path)?,
            last_write: Duration::ZERO,
        })
    }

    pub fn record_if_due(
        &mut self,
        elapsed: Duration,
        wall_seconds: u64,
    ) -> rusqlite::Result<bool> {
        if elapsed.saturating_sub(self.last_write) < Duration::from_secs(60) {
            return Ok(false);
        }
        let sample = self.counters.take(wall_seconds);
        if let Err(error) = self.store.record(&sample) {
            self.counters.restore(&sample);
            return Err(error);
        }
        self.last_write = elapsed;
        Ok(true)
    }
}

impl PerfStore {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;",
        )?;
        connection.execute_batch(include_str!("perf_schema.sql"))?;
        Ok(Self { connection })
    }

    pub fn record(&mut self, sample: &MinuteSample) -> rusqlite::Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT OR REPLACE INTO perf_minute VALUES (
                ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20
            )",
            params![
                sample.wall_seconds as i64,
                sample.process_cpu_seconds,
                sample.state_seconds[0],
                sample.state_seconds[1],
                sample.state_seconds[2],
                sample.state_seconds[3],
                sample.state_seconds[4],
                sample.state_cpu_seconds[0],
                sample.state_cpu_seconds[1],
                sample.state_cpu_seconds[2],
                sample.state_cpu_seconds[3],
                sample.state_cpu_seconds[4],
                sample.draw_count as i64,
                sample.draw_total_ms,
                sample.sampler_runs as i64,
                sample.sampler_total_ms,
                sample.history_write_ms,
                sample.refresh_interval_seconds as i64,
                sample.app_version,
                sample.app_commit,
            ],
        )?;
        tx.execute(
            "DELETE FROM perf_minute WHERE wall_seconds < ?1",
            [sample.wall_seconds.saturating_sub(RETENTION_SECONDS) as i64],
        )?;
        tx.commit()
    }
}

pub fn process_cpu_seconds() -> Option<f64> {
    #[cfg(target_os = "macos")]
    {
        use std::mem::MaybeUninit;

        let mut usage = MaybeUninit::<libc::rusage_info_v2>::zeroed();
        let result = unsafe {
            libc::proc_pid_rusage(
                libc::getpid(),
                libc::RUSAGE_INFO_V2,
                usage.as_mut_ptr().cast(),
            )
        };
        if result != 0 {
            return None;
        }
        let usage = unsafe { usage.assume_init() };
        let mut timebase = Timebase { numer: 0, denom: 0 };
        if unsafe { mach_timebase_info(&mut timebase) } != 0 || timebase.denom == 0 {
            return None;
        }
        let ticks = usage.ri_user_time.saturating_add(usage.ri_system_time);
        Some(ticks as f64 * timebase.numer as f64 / timebase.denom as f64 / 1_000_000_000.)
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(target_os = "macos")]
pub fn window_state() -> WindowState {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSWindowOcclusionState};

    let Some(mtm) = MainThreadMarker::new() else {
        return WindowState::Unknown;
    };
    let app = NSApplication::sharedApplication(mtm);
    let Some(window) = app
        .windows()
        .iter()
        .find(|window| window.title().to_string() == "gtop")
    else {
        return WindowState::Unknown;
    };
    if window.isMiniaturized() {
        WindowState::Minimized
    } else if !window.isVisible() {
        WindowState::Hidden
    } else if window
        .occlusionState()
        .contains(NSWindowOcclusionState::Visible)
    {
        WindowState::Visible
    } else {
        WindowState::Occluded
    }
}

#[cfg(not(target_os = "macos"))]
pub fn window_state() -> WindowState {
    WindowState::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_return_interval_totals_and_reset_after_take() {
        let counters = PerfCounters::default();
        counters.add_window_time(WindowState::Visible, Duration::from_secs(42));
        counters.add_process_cpu(WindowState::Visible, Duration::from_millis(800));
        counters.add_draws(12, Duration::from_millis(30));
        counters.add_sampler(Duration::from_millis(25));
        counters.add_sampler(Duration::from_millis(35));
        counters.add_history_write(Duration::from_millis(5));
        counters.set_refresh_interval(5);

        let sample = counters.take(1_800_000_000);
        assert_eq!(sample.wall_seconds, 1_800_000_000);
        assert_eq!(sample.process_cpu_seconds, 0.8);
        assert_eq!(sample.state_seconds[WindowState::Visible as usize], 42.);
        assert_eq!(sample.state_cpu_seconds[WindowState::Visible as usize], 0.8);
        assert_eq!(sample.draw_count, 12);
        assert_eq!(sample.draw_total_ms, 30.);
        assert_eq!(sample.sampler_runs, 2);
        assert_eq!(sample.sampler_total_ms, 60.);
        assert_eq!(sample.history_write_ms, 5.);
        assert_eq!(sample.refresh_interval_seconds, 5);

        let empty = counters.take(1_800_000_060);
        assert_eq!(empty.draw_count, 0);
        assert_eq!(empty.sampler_runs, 0);
        assert_eq!(empty.state_seconds.iter().sum::<f64>(), 0.);
    }

    #[test]
    fn store_writes_one_row_and_removes_data_older_than_thirty_days() {
        let dir = std::env::temp_dir().join(format!("gtop-perf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut store = PerfStore::open(&dir.join("perf.sqlite")).unwrap();
        let mut old = PerfCounters::default().take(100);
        old.wall_seconds = 100;
        store.record(&old).unwrap();
        let mut recent = PerfCounters::default().take(RETENTION_SECONDS + 101);
        recent.wall_seconds = RETENTION_SECONDS + 101;
        store.record(&recent).unwrap();
        let count: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM perf_minute", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
        let timestamp: i64 = store
            .connection
            .query_row("SELECT wall_seconds FROM perf_minute", [], |row| row.get(0))
            .unwrap();
        assert_eq!(timestamp, (RETENTION_SECONDS + 101) as i64);
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }
}
