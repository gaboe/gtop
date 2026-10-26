use rusqlite::{params, Connection, TransactionBehavior};
use std::{
    collections::VecDeque,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
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
    pub interval_start_seconds: f64,
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
    window: Mutex<WindowTotals>,
    draw_count: AtomicU64,
    draw_total_ns: AtomicU64,
    sampler_runs: AtomicU64,
    sampler_total_ns: AtomicU64,
    history_write_ns: AtomicU64,
    refresh_interval_seconds: AtomicU64,
}

#[derive(Default)]
struct WindowTotals {
    seconds: [f64; WINDOW_STATE_COUNT],
    cpu_seconds: [f64; WINDOW_STATE_COUNT],
}

pub struct CpuBaseline(Option<f64>);

impl CpuBaseline {
    pub fn new(cpu: Option<f64>) -> Self {
        Self(cpu)
    }

    pub fn sample(&mut self, cpu: Option<f64>) -> Duration {
        let previous = std::mem::replace(&mut self.0, cpu);
        match (previous, cpu) {
            (Some(previous), Some(current)) if current >= previous => {
                Duration::from_secs_f64(current - previous)
            }
            _ => Duration::ZERO,
        }
    }
}

impl Default for PerfCounters {
    fn default() -> Self {
        Self {
            window: Mutex::default(),
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
    pub fn add_window_sample(&self, state: WindowState, elapsed: Duration, cpu: Duration) {
        let mut window = self.window.lock().unwrap();
        window.seconds[state as usize] += elapsed.as_secs_f64();
        window.cpu_seconds[state as usize] += cpu.as_secs_f64();
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
        let window = std::mem::take(&mut *self.window.lock().unwrap());
        let state_seconds = window.seconds;
        let state_cpu_seconds = window.cpu_seconds;
        let process_cpu_seconds = state_cpu_seconds.iter().sum();
        MinuteSample {
            wall_seconds,
            interval_start_seconds: wall_seconds as f64 - state_seconds.iter().sum::<f64>(),
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
}

pub struct PerfStore {
    connection: Connection,
}

pub struct PerfRecorder {
    counters: Arc<PerfCounters>,
    store: PerfStore,
    last_write: Duration,
    pending: VecDeque<MinuteSample>,
}

impl PerfRecorder {
    pub fn open(path: &Path, counters: Arc<PerfCounters>) -> rusqlite::Result<Self> {
        Ok(Self {
            counters,
            store: PerfStore::open(path)?,
            last_write: Duration::ZERO,
            pending: VecDeque::new(),
        })
    }

    pub fn record_if_due(
        &mut self,
        elapsed: Duration,
        wall_seconds: u64,
    ) -> rusqlite::Result<bool> {
        let due = elapsed.saturating_sub(self.last_write) >= Duration::from_secs(60);
        if due {
            let mut sample = self.counters.take(wall_seconds);
            sample.interval_start_seconds =
                wall_seconds as f64 - elapsed.saturating_sub(self.last_write).as_secs_f64();
            self.pending.push_back(sample);
            self.last_write = elapsed;
        }
        while let Some(sample) = self.pending.front() {
            self.store.record(sample)?;
            self.pending.pop_front();
        }
        Ok(due)
    }
}

impl PerfStore {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        }
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;",
        )?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let legacy = tx
            .prepare("PRAGMA table_info(perf_minute)")?
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .iter()
            .any(|(name, pk)| name == "wall_seconds" && *pk != 0);
        if legacy {
            tx.execute_batch("ALTER TABLE perf_minute RENAME TO perf_minute_legacy;")?;
            tx.execute_batch(include_str!("perf_schema.sql"))?;
            tx.execute_batch(
                "INSERT INTO perf_minute SELECT *, NULL,
                wall_seconds - visible_seconds - occluded_seconds - hidden_seconds
                - minimized_seconds - unknown_seconds FROM perf_minute_legacy;
                DROP TABLE perf_minute_legacy;",
            )?;
        } else {
            tx.execute_batch(include_str!("perf_schema.sql"))?;
        }
        tx.execute_batch(
            "CREATE INDEX IF NOT EXISTS perf_minute_wall_seconds ON perf_minute(wall_seconds);",
        )?;
        tx.commit()?;
        Ok(Self { connection })
    }

    pub fn record(&mut self, sample: &MinuteSample) -> rusqlite::Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO perf_minute VALUES (
                ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,NULL,?21
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
                sample.interval_start_seconds,
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
    fn concurrent_samples_never_separate_cpu_from_duration() {
        let counters = Arc::new(PerfCounters::default());
        let producer = counters.clone();
        let thread = std::thread::spawn(move || {
            for _ in 0..10000 {
                producer.add_window_sample(
                    WindowState::Visible,
                    Duration::from_secs(1),
                    Duration::from_millis(100),
                );
            }
        });
        let mut total = 0.;
        while !thread.is_finished() {
            let sample = counters.take(10000);
            assert!((sample.state_cpu_seconds[0] - sample.state_seconds[0] / 10.).abs() < 1e-8);
            total += sample.state_seconds[0];
        }
        thread.join().unwrap();
        let sample = counters.take(10000);
        assert!((sample.state_cpu_seconds[0] - sample.state_seconds[0] / 10.).abs() < 1e-8);
        assert_eq!(total + sample.state_seconds[0], 10000.);
    }

    #[test]
    fn first_cpu_interval_uses_initial_baseline() {
        let mut baseline = CpuBaseline::new(Some(3.));
        assert_eq!(baseline.sample(Some(3.25)), Duration::from_millis(250));
        assert_eq!(baseline.sample(None), Duration::ZERO);
        assert_eq!(baseline.sample(Some(4.)), Duration::ZERO);
        assert_eq!(baseline.sample(Some(3.)), Duration::ZERO);
    }

    #[test]
    fn collisions_and_clock_rollback_keep_all_samples() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("perf.sqlite");
        let mut first = PerfStore::open(&path).unwrap();
        let mut second = PerfStore::open(&path).unwrap();
        for (index, wall, cpu) in [(0, 10000, 1.), (1, 10000, 9.), (0, 9940, 2.)] {
            let store = if index == 0 { &mut first } else { &mut second };
            let mut sample = PerfCounters::default().take(wall);
            sample.process_cpu_seconds = cpu;
            store.record(&sample).unwrap();
        }
        let result: (i64, f64) = first
            .connection
            .query_row(
                "SELECT count(*), sum(process_cpu_seconds) FROM perf_minute",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(result, (3, 12.));
    }

    #[test]
    fn locked_write_keeps_original_minutes_and_timestamps() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("perf.sqlite");
        let counters = Arc::new(PerfCounters::default());
        let mut recorder = PerfRecorder::open(&path, counters.clone()).unwrap();
        recorder
            .store
            .connection
            .busy_timeout(Duration::ZERO)
            .unwrap();
        let blocker = Connection::open(&path).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        counters.add_window_sample(
            WindowState::Visible,
            Duration::from_secs(60),
            Duration::from_secs(6),
        );
        assert!(recorder
            .record_if_due(Duration::from_secs(60), 3599)
            .is_err());
        blocker.execute_batch("ROLLBACK").unwrap();
        counters.add_window_sample(
            WindowState::Visible,
            Duration::from_secs(60),
            Duration::from_millis(600),
        );
        assert!(recorder
            .record_if_due(Duration::from_secs(120), 3659)
            .unwrap());
        let samples = recorder.store.connection.prepare(
            "SELECT wall_seconds, interval_start_seconds, visible_seconds, cpu_visible_seconds FROM perf_minute ORDER BY sample_id").unwrap()
            .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?, row.get::<_, f64>(2)?, row.get::<_, f64>(3)?))).unwrap()
            .collect::<rusqlite::Result<Vec<_>>>().unwrap();
        assert_eq!(
            samples,
            vec![(3599, 3539., 60., 6.), (3659, 3599., 60., 0.6)]
        );
    }

    #[test]
    fn legacy_database_migration_preserves_rows() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("perf.sqlite");
        let legacy = include_str!("perf_schema.sql")
            .replace(
                "wall_seconds INTEGER NOT NULL",
                "wall_seconds INTEGER PRIMARY KEY",
            )
            .replace(
                ",\n    sample_id INTEGER PRIMARY KEY,\n    interval_start_seconds REAL NOT NULL",
                "",
            );
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(&legacy).unwrap();
        connection
            .execute_batch(
                "INSERT INTO perf_minute VALUES (3600,6,60,0,0,0,0,6,0,0,0,0,0,0,0,0,0,3,'1','a')",
            )
            .unwrap();
        drop(connection);
        let mut store = PerfStore::open(&path).unwrap();
        store.record(&PerfCounters::default().take(3600)).unwrap();
        let count: i64 = store
            .connection
            .query_row("SELECT count(*) FROM perf_minute", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
        let start: f64 = store
            .connection
            .query_row(
                "SELECT interval_start_seconds FROM perf_minute WHERE sample_id=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(start, 3540.);
    }

    #[test]
    fn counters_return_interval_totals_and_reset_after_take() {
        let counters = PerfCounters::default();
        counters.add_window_sample(
            WindowState::Visible,
            Duration::from_secs(42),
            Duration::from_millis(800),
        );
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
