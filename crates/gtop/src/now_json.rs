use serde::Serialize;
use std::{
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream},
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const PROBE_INTERVAL: Duration = Duration::from_secs(15);
const PROBE_TIMEOUT: Duration = Duration::from_secs(4);
const PROBE_WINDOW: Duration = Duration::from_secs(5 * 60);
const PROBE_LIMIT: usize = 20;
const LATENCY_PROBES: usize = 4;
const TARGET: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(1, 1, 1, 1), 443));

#[derive(Clone, Copy, Debug)]
pub(crate) struct SampledMetrics {
    pub(crate) time: u64,
    pub(crate) interval_s: u64,
    pub(crate) sampled_at: Instant,
    pub(crate) cpu_percent: f64,
    pub(crate) memory_used_bytes: u64,
    pub(crate) memory_total_bytes: u64,
    pub(crate) disk_free_bytes: Option<u64>,
    pub(crate) disk_total_bytes: Option<u64>,
    pub(crate) disk_io_totals: Option<(u64, u64)>,
    pub(crate) rx_bytes_per_s: u64,
    pub(crate) tx_bytes_per_s: u64,
    pub(crate) temperature_c: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Outcome {
    Reply(Duration),
    Lost,
}

#[derive(Clone, Default)]
struct ProbeReading {
    loss: Vec<(u64, Outcome)>,
    latency: Vec<(u64, Duration)>,
}

impl ProbeReading {
    fn record(&mut self, at: u64, outcome: Outcome) {
        self.loss.push((at, outcome));
        if let Outcome::Reply(duration) = outcome {
            self.latency.push((at, duration));
        }
        self.expire(at);
    }

    fn expire(&mut self, now: u64) {
        self.loss
            .retain(|(at, _)| *at <= now && now - *at <= PROBE_WINDOW.as_secs());
        self.loss
            .drain(..self.loss.len().saturating_sub(PROBE_LIMIT));
        self.latency
            .retain(|(at, _)| *at <= now && now - *at <= PROBE_WINDOW.as_secs());
        self.latency
            .drain(..self.latency.len().saturating_sub(LATENCY_PROBES));
    }

    fn latency_ms(&self) -> Option<f64> {
        let mut successful: Vec<_> = self
            .latency
            .iter()
            .map(|(_, duration)| duration.as_secs_f64() * 1000.)
            .collect();
        successful.sort_by(f64::total_cmp);
        successful.get(successful.len() / 2).copied()
    }

    fn loss_percent(&self) -> Option<f64> {
        (!self.loss.is_empty()).then(|| {
            self.loss
                .iter()
                .filter(|(_, outcome)| *outcome == Outcome::Lost)
                .count() as f64
                / self.loss.len() as f64
                * 100.
        })
    }
}

#[derive(Serialize)]
struct NowSnapshot {
    version: u8,
    time: u64,
    interval_s: u8,
    pid: u32,
    commit: &'static str,
    cpu: Cpu,
    memory: Memory,
    ssd: Ssd,
    net: Net,
}

#[derive(Serialize)]
struct Cpu {
    percent: f64,
}

#[derive(Serialize)]
struct Memory {
    used_bytes: u64,
    total_bytes: u64,
    percent: f64,
}

#[derive(Serialize)]
struct Ssd {
    free_bytes: Option<u64>,
    total_bytes: Option<u64>,
    read_bytes_per_s: Option<f64>,
    write_bytes_per_s: Option<f64>,
    temperature_c: Option<f64>,
}

#[derive(Serialize)]
struct Net {
    rx_bytes_per_s: u64,
    tx_bytes_per_s: u64,
    latency_ms: Option<f64>,
    loss_percent: Option<f64>,
    probe: &'static str,
}

#[cfg(test)]
fn snapshot_at(metrics: SampledMetrics, reading: &ProbeReading, now: u64) -> NowSnapshot {
    snapshot_at_with_disk_io_rates(metrics, reading, now, None)
}

fn snapshot_at_with_disk_io_rates(
    metrics: SampledMetrics,
    reading: &ProbeReading,
    now: u64,
    disk_io_rates: Option<(f64, f64)>,
) -> NowSnapshot {
    let mut reading = reading.clone();
    reading.expire(now);
    NowSnapshot {
        version: 1,
        time: metrics.time,
        interval_s: metrics.interval_s.min(u8::MAX as u64) as u8,
        pid: std::process::id(),
        commit: option_env!("GTOP_GIT_COMMIT").unwrap_or("unknown"),
        cpu: Cpu {
            percent: metrics.cpu_percent,
        },
        memory: Memory {
            used_bytes: metrics.memory_used_bytes,
            total_bytes: metrics.memory_total_bytes,
            percent: (metrics.memory_used_bytes as f32 / metrics.memory_total_bytes.max(1) as f32
                * 100.) as f64,
        },
        ssd: Ssd {
            free_bytes: metrics.disk_free_bytes,
            total_bytes: metrics.disk_total_bytes,
            read_bytes_per_s: disk_io_rates.map(|(read, _)| read),
            write_bytes_per_s: disk_io_rates.map(|(_, write)| write),
            temperature_c: metrics.temperature_c,
        },
        net: Net {
            rx_bytes_per_s: metrics.rx_bytes_per_s,
            tx_bytes_per_s: metrics.tx_bytes_per_s,
            latency_ms: reading.latency_ms(),
            loss_percent: reading.loss_percent(),
            probe: "tcp 1.1.1.1:443",
        },
    }
}

#[cfg(test)]
fn snapshot(metrics: SampledMetrics, reading: &ProbeReading) -> NowSnapshot {
    snapshot_at(metrics, reading, unix_seconds())
}

#[derive(Default)]
struct DiskIoRate {
    previous: Option<(Instant, Option<(u64, u64)>)>,
}

impl DiskIoRate {
    fn update(
        &mut self,
        sampled_at: Instant,
        totals: Option<(u64, u64)>,
        interval_s: u64,
    ) -> Option<(f64, f64)> {
        let (previous_at, previous_totals) = self.previous.replace((sampled_at, totals))?;
        let (previous_read, previous_write) = previous_totals?;
        let (current_read, current_write) = totals?;
        let elapsed = sampled_at.duration_since(previous_at);
        if elapsed.is_zero()
            || elapsed > Duration::from_secs(interval_s.saturating_mul(3))
            || current_read < previous_read
            || current_write < previous_write
        {
            return None;
        }
        let seconds = elapsed.as_secs_f64();
        Some((
            (current_read - previous_read) as f64 / seconds,
            (current_write - previous_write) as f64 / seconds,
        ))
    }
}

fn write_atomic(directory: &Path, value: &NowSnapshot) -> io::Result<()> {
    std::fs::create_dir_all(directory)?;
    let file = directory.join("now.json");
    let temporary = directory.join(format!(".now.{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    let result = std::fs::write(&temporary, bytes).and_then(|()| std::fs::rename(&temporary, file));
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

fn probe_once(connector: impl FnOnce(SocketAddr, Duration) -> io::Result<Duration>) -> Outcome {
    match connector(TARGET, PROBE_TIMEOUT) {
        Ok(duration) => Outcome::Reply(duration),
        Err(_) => Outcome::Lost,
    }
}

fn connect(target: SocketAddr, timeout: Duration) -> io::Result<Duration> {
    let started = Instant::now();
    TcpStream::connect_timeout(&target, timeout)?;
    Ok(started.elapsed())
}

fn probe_loop(
    stop: Arc<StopSignal>,
    reading: Arc<Mutex<ProbeReading>>,
    interval: Duration,
    connector: impl Fn(SocketAddr, Duration) -> io::Result<Duration>,
) {
    loop {
        if !stop.wait(Duration::ZERO) {
            break;
        }
        let started = Instant::now();
        let outcome = probe_once(&connector);
        reading
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .record(unix_seconds(), outcome);
        if !stop.wait(time_until_next_probe(started, interval, Instant::now())) {
            break;
        }
    }
}

fn time_until_next_probe(started: Instant, interval: Duration, now: Instant) -> Duration {
    (started + interval).saturating_duration_since(now)
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

struct StopSignal {
    stopped: Mutex<bool>,
    wake: Condvar,
}

impl StopSignal {
    fn new() -> Self {
        Self {
            stopped: Mutex::new(false),
            wake: Condvar::new(),
        }
    }

    fn wait(&self, duration: Duration) -> bool {
        let stopped = self
            .stopped
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if *stopped {
            return false;
        }
        let (stopped, _) = self
            .wake
            .wait_timeout(stopped, duration)
            .unwrap_or_else(|error| error.into_inner());
        !*stopped
    }

    fn stop(&self) {
        *self
            .stopped
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = true;
        self.wake.notify_all();
    }
}

pub(crate) struct Writer {
    updates: mpsc::Sender<SampledMetrics>,
    stop: Arc<StopSignal>,
    _workers: Vec<JoinHandle<()>>,
}

impl Writer {
    pub(crate) fn start(directory: PathBuf) -> io::Result<Self> {
        Self::start_with(directory, connect, PROBE_INTERVAL)
    }

    fn start_with(
        directory: PathBuf,
        connector: impl Fn(SocketAddr, Duration) -> io::Result<Duration> + Send + 'static,
        probe_interval: Duration,
    ) -> io::Result<Self> {
        let (updates, receiver) = mpsc::channel::<SampledMetrics>();
        let reading = Arc::new(Mutex::new(ProbeReading::default()));
        let stop = Arc::new(StopSignal::new());

        let probe_stop = stop.clone();
        let probe_reading = reading.clone();
        let probe = thread::Builder::new()
            .name("gtop-network-probe".into())
            .spawn(move || probe_loop(probe_stop, probe_reading, probe_interval, connector))?;

        let writer_stop = stop.clone();
        let writer_reading = reading;
        let writer = match thread::Builder::new()
            .name("gtop-now-json".into())
            .spawn(move || {
                let mut disk_io_rate = DiskIoRate::default();
                while writer_stop.wait(Duration::ZERO) {
                    let metrics = match receiver.recv() {
                        Ok(metrics) => metrics,
                        Err(_) => break,
                    };
                    let reading = writer_reading
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    let disk_io_rates = disk_io_rate.update(
                        metrics.sampled_at,
                        metrics.disk_io_totals,
                        metrics.interval_s,
                    );
                    let snapshot = snapshot_at_with_disk_io_rates(
                        metrics,
                        &reading,
                        unix_seconds(),
                        disk_io_rates,
                    );
                    if let Err(error) = write_atomic(&directory, &snapshot) {
                        eprintln!("Current metrics could not be saved: {error}");
                    }
                }
            }) {
            Ok(writer) => writer,
            Err(error) => {
                stop.stop();
                let _ = probe.join();
                return Err(error);
            }
        };

        Ok(Self {
            updates,
            stop,
            _workers: vec![probe, writer],
        })
    }

    pub(crate) fn update(&self, metrics: SampledMetrics) {
        let _ = self.updates.send(metrics);
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.stop.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    fn metrics() -> SampledMetrics {
        SampledMetrics {
            time: 1_800_000_000,
            interval_s: 2,
            sampled_at: Instant::now(),
            cpu_percent: 12.5,
            memory_used_bytes: 500,
            memory_total_bytes: 1000,
            disk_free_bytes: Some(2000),
            disk_total_bytes: Some(4000),
            disk_io_totals: Some((0, 0)),
            rx_bytes_per_s: 10,
            tx_bytes_per_s: 20,
            temperature_c: None,
        }
    }

    #[test]
    fn serialization_matches_the_version_one_format() {
        let mut reading = ProbeReading::default();
        reading.record(1_800_000_000, Outcome::Reply(Duration::from_millis(25)));
        let value = snapshot_at_with_disk_io_rates(
            metrics(),
            &reading,
            1_800_000_000,
            Some((123.0, 456.0)),
        );
        let json = serde_json::to_string(&value).unwrap();
        let expected = format!(
            r#"{{"version":1,"time":1800000000,"interval_s":2,"pid":{},"commit":"{}","cpu":{{"percent":12.5}},"memory":{{"used_bytes":500,"total_bytes":1000,"percent":50.0}},"ssd":{{"free_bytes":2000,"total_bytes":4000,"read_bytes_per_s":123.0,"write_bytes_per_s":456.0,"temperature_c":null}},"net":{{"rx_bytes_per_s":10,"tx_bytes_per_s":20,"latency_ms":25.0,"loss_percent":0.0,"probe":"tcp 1.1.1.1:443"}}}}"#,
            std::process::id(),
            option_env!("GTOP_GIT_COMMIT").unwrap_or("unknown")
        );
        assert_eq!(json, expected);
        println!("SAMPLE_NOW_JSON={json}");
    }

    #[test]
    fn disk_io_rates_are_null_for_the_first_sample() {
        let mut rate = DiskIoRate::default();
        let start = Instant::now();
        assert_eq!(rate.update(start, Some((100, 200)), 2), None);
    }

    #[test]
    fn disk_io_rates_use_cumulative_counter_deltas() {
        let mut rate = DiskIoRate::default();
        let start = Instant::now();
        assert_eq!(rate.update(start, Some((100, 200)), 2), None);
        assert_eq!(
            rate.update(start + Duration::from_secs(2), Some((500, 800)), 2),
            Some((200.0, 300.0))
        );
    }

    #[test]
    fn disk_io_counter_decrease_resets_both_rates() {
        let mut rate = DiskIoRate::default();
        let start = Instant::now();
        assert_eq!(rate.update(start, Some((100, 200)), 2), None);
        assert_eq!(
            rate.update(start + Duration::from_secs(1), Some((150, 199)), 2),
            None
        );
        assert_eq!(
            rate.update(start + Duration::from_secs(2), Some((170, 239)), 2),
            Some((20.0, 40.0))
        );
    }

    #[test]
    fn disk_io_rates_are_null_after_a_long_gap() {
        let mut rate = DiskIoRate::default();
        let start = Instant::now();
        assert_eq!(rate.update(start, Some((100, 200)), 2), None);
        assert_eq!(
            rate.update(start + Duration::from_secs(7), Some((800, 900)), 2),
            None
        );
        assert_eq!(
            rate.update(start + Duration::from_secs(8), Some((900, 1000)), 2),
            Some((100.0, 100.0))
        );
    }

    #[test]
    fn disk_io_rates_are_null_for_zero_elapsed_time() {
        let mut rate = DiskIoRate::default();
        let sample = Instant::now();
        assert_eq!(rate.update(sample, Some((100, 200)), 2), None);
        assert_eq!(rate.update(sample, Some((200, 300)), 2), None);
        assert_eq!(
            rate.update(sample + Duration::from_secs(1), Some((250, 350)), 2),
            Some((50.0, 50.0))
        );
    }

    #[test]
    fn disk_io_rates_accept_a_gap_of_exactly_three_intervals() {
        let mut rate = DiskIoRate::default();
        let start = Instant::now();
        assert_eq!(rate.update(start, Some((100, 200)), 2), None);
        assert_eq!(
            rate.update(start + Duration::from_secs(6), Some((700, 800)), 2),
            Some((100.0, 100.0))
        );
    }

    #[test]
    fn unchanged_disk_io_counters_give_zero_rates() {
        let mut rate = DiskIoRate::default();
        let start = Instant::now();
        assert_eq!(rate.update(start, Some((100, 200)), 2), None);
        assert_eq!(
            rate.update(start + Duration::from_secs(2), Some((100, 200)), 2),
            Some((0.0, 0.0))
        );
    }

    #[test]
    fn missing_disk_io_totals_reset_the_rate_baseline() {
        let mut rate = DiskIoRate::default();
        let start = Instant::now();
        assert_eq!(rate.update(start, None, 2), None);
        assert_eq!(
            rate.update(start + Duration::from_secs(1), Some((100, 200)), 2),
            None
        );
        assert_eq!(
            rate.update(start + Duration::from_secs(2), Some((150, 260)), 2),
            Some((50.0, 60.0))
        );
        assert_eq!(rate.update(start + Duration::from_secs(3), None, 2), None);
        assert_eq!(
            rate.update(start + Duration::from_secs(4), Some((900, 900)), 2),
            None
        );
    }

    #[test]
    fn atomic_write_exposes_only_complete_files() {
        let directory = tempfile::tempdir().unwrap();
        let first = snapshot(metrics(), &ProbeReading::default());
        write_atomic(directory.path(), &first).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let reader_stop = stop.clone();
        let path = directory.path().join("now.json");
        let reader = thread::spawn(move || {
            while !reader_stop.load(Ordering::Relaxed) {
                let bytes = std::fs::read(&path).unwrap();
                let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(json["version"], 1);
                assert!(
                    json["cpu"]["percent"].as_f64().unwrap() == 12.5
                        || json["cpu"]["percent"].as_f64().unwrap() == 87.5
                );
            }
        });
        for _ in 0..100 {
            let mut changed = metrics();
            changed.cpu_percent = 87.5;
            write_atomic(
                directory.path(),
                &snapshot(changed, &ProbeReading::default()),
            )
            .unwrap();
            write_atomic(directory.path(), &first).unwrap();
        }
        stop.store(true, Ordering::Relaxed);
        reader.join().unwrap();
    }

    #[test]
    fn probe_statistics_use_recent_answers_and_a_five_minute_window() {
        let mut reading = ProbeReading::default();
        assert_eq!(reading.latency_ms(), None);
        assert_eq!(reading.loss_percent(), None);
        for (at, millis) in [(0, 900), (1, 30), (2, 500), (3, 40), (4, 35)] {
            reading.record(1_000 + at, Outcome::Reply(Duration::from_millis(millis)));
        }
        assert_eq!(reading.latency_ms(), Some(40.));
        reading.record(1_005, Outcome::Lost);
        assert_eq!(reading.loss_percent(), Some(1. / 6. * 100.));
        let expired = snapshot_at(metrics(), &reading, 1_306);
        let json = serde_json::to_value(expired).unwrap();
        assert_eq!(json["net"]["latency_ms"], serde_json::Value::Null);
        assert_eq!(json["net"]["loss_percent"], serde_json::Value::Null);
    }

    #[test]
    fn timeout_errors_count_as_failed_attempts() {
        fn injected_connector(target: SocketAddr, timeout: Duration) -> io::Result<Duration> {
            assert_eq!(target, TARGET);
            assert_eq!(timeout, PROBE_TIMEOUT);
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{timeout:?}"),
            ))
        }

        let outcome = probe_once(injected_connector);
        assert_eq!(outcome, Outcome::Lost);
        let mut reading = ProbeReading::default();
        reading.record(10, Outcome::Lost);
        assert_eq!(reading.loss_percent(), Some(100.));
    }

    #[test]
    fn probe_window_keeps_at_most_twenty_attempts() {
        let mut reading = ProbeReading::default();
        for at in 0..25 {
            reading.record(at, Outcome::Reply(Duration::from_millis(at)));
        }
        assert_eq!(reading.loss.len(), PROBE_LIMIT);
        assert_eq!(reading.latency_ms(), Some(23.));
    }

    #[test]
    fn latency_survives_a_full_window_of_failures() {
        let mut reading = ProbeReading::default();
        for (at, millis) in [(1, 10), (2, 20), (3, 30), (4, 40)] {
            reading.record(at, Outcome::Reply(Duration::from_millis(millis)));
        }
        for at in 5..25 {
            reading.record(at, Outcome::Lost);
        }
        assert_eq!(reading.loss.len(), PROBE_LIMIT);
        assert_eq!(reading.latency_ms(), Some(30.));
    }

    #[test]
    fn snapshot_reports_the_active_sampling_interval() {
        let mut sample = metrics();
        sample.interval_s = 10;
        let value = snapshot_at(sample, &ProbeReading::default(), sample.time);
        let directory = tempfile::tempdir().unwrap();
        write_atomic(directory.path(), &value).unwrap();
        let contents: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.path().join("now.json")).unwrap())
                .unwrap();
        assert_eq!(contents["interval_s"], 10);
    }

    #[test]
    fn publication_keeps_probe_results_for_five_minutes() {
        let mut reading = ProbeReading::default();
        reading.record(1_000, Outcome::Reply(Duration::from_millis(25)));
        let json = serde_json::to_value(snapshot_at(metrics(), &reading, 1_300)).unwrap();
        assert_eq!(json["net"]["latency_ms"], 25.0);
        assert_eq!(json["net"]["loss_percent"], 0.0);
    }

    #[test]
    fn publication_expires_old_probe_results() {
        let mut reading = ProbeReading::default();
        reading.record(1_000, Outcome::Reply(Duration::from_millis(25)));
        reading.record(1_000, Outcome::Lost);
        let value = snapshot_at(metrics(), &reading, 1_301);
        let json = serde_json::to_value(value).unwrap();
        assert_eq!(json["net"]["latency_ms"], serde_json::Value::Null);
        assert_eq!(json["net"]["loss_percent"], serde_json::Value::Null);
    }

    const BOUND: Duration = Duration::from_secs(10);

    fn within_bound<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
        let (done, finished) = mpsc::channel();
        thread::spawn(move || {
            let _ = done.send(work());
        });
        finished
            .recv_timeout(BOUND)
            .expect("work finished within the bound")
    }

    #[test]
    fn stopped_probe_does_not_connect() {
        let stop = Arc::new(StopSignal::new());
        stop.stop();
        let called = Arc::new(AtomicBool::new(false));
        let connector_called = called.clone();
        within_bound(move || {
            probe_loop(
                stop,
                Arc::new(Mutex::new(ProbeReading::default())),
                Duration::ZERO,
                move |_, _| {
                    connector_called.store(true, Ordering::Relaxed);
                    Ok(Duration::ZERO)
                },
            )
        });
        assert!(!called.load(Ordering::Relaxed));
    }

    #[test]
    fn probe_loop_records_every_probe_until_stopped() {
        let stop = Arc::new(StopSignal::new());
        let reading = Arc::new(Mutex::new(ProbeReading::default()));
        let calls = Arc::new(AtomicUsize::new(0));
        let (loop_stop, loop_reading, loop_calls) = (stop.clone(), reading.clone(), calls.clone());
        within_bound(move || {
            probe_loop(loop_stop, loop_reading, Duration::ZERO, move |_, _| {
                if loop_calls.fetch_add(1, Ordering::Relaxed) == 2 {
                    stop.stop();
                }
                Ok(Duration::from_millis(5))
            })
        });
        assert_eq!(calls.load(Ordering::Relaxed), 3);
        let reading = reading.lock().unwrap();
        assert_eq!(reading.loss.len(), 3);
        assert_eq!(reading.loss_percent(), Some(0.));
        assert_eq!(reading.latency_ms(), Some(5.));
    }

    #[test]
    fn next_probe_waits_out_the_rest_of_the_interval() {
        let started = Instant::now();
        assert_eq!(
            time_until_next_probe(started, PROBE_INTERVAL, started + Duration::from_secs(4)),
            Duration::from_secs(11)
        );
        assert_eq!(
            time_until_next_probe(started, PROBE_INTERVAL, started + PROBE_TIMEOUT * 5),
            Duration::ZERO
        );
    }

    #[test]
    fn stop_signal_times_out_while_running_and_returns_at_once_when_stopped() {
        let signal = Arc::new(StopSignal::new());
        assert!(signal.wait(Duration::from_millis(10)));
        let waiter = signal.clone();
        let (started, waiting) = mpsc::channel();
        let (done, finished) = mpsc::channel();
        thread::spawn(move || {
            started.send(()).unwrap();
            let _ = done.send(waiter.wait(Duration::from_secs(600)));
        });
        waiting.recv_timeout(BOUND).unwrap();
        signal.stop();
        assert_eq!(finished.recv_timeout(BOUND), Ok(false));
        assert!(!signal.wait(Duration::from_millis(10)));
    }

    #[test]
    fn unix_seconds_reads_the_system_clock() {
        let seconds = || {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
        };
        let before = seconds();
        let now = unix_seconds();
        assert!(before <= now && now <= seconds());
    }

    #[test]
    fn connect_succeeds_on_an_open_port_and_fails_on_a_closed_one() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let open = listener.local_addr().unwrap();
        assert!(connect(open, BOUND).is_ok());
        drop(listener);
        assert!(connect(open, BOUND).is_err());
    }

    #[test]
    fn writer_publishes_updates_and_stops_probing_when_dropped() {
        let directory = tempfile::tempdir().unwrap();
        let (probed, probes) = mpsc::channel();
        let writer = Writer::start_with(
            directory.path().to_path_buf(),
            move |_, _| {
                let _ = probed.send(());
                Ok(Duration::from_millis(5))
            },
            Duration::from_secs(3_600),
        )
        .unwrap();
        probes.recv_timeout(BOUND).unwrap();
        let mut sample = metrics();
        sample.time = 1_900_000_000;
        writer.update(sample);
        let path = directory.path().join("now.json");
        let deadline = Instant::now() + BOUND;
        let published = loop {
            let json = std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
            if let Some(json) = json {
                break json;
            }
            assert!(Instant::now() < deadline, "now.json was not written");
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(published["time"], 1_900_000_000);
        drop(writer);
        assert_eq!(
            probes.recv_timeout(BOUND),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
    }

    #[test]
    fn failed_rename_removes_temporary_file() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("now.json")).unwrap();
        assert!(write_atomic(
            directory.path(),
            &snapshot(metrics(), &ProbeReading::default())
        )
        .is_err());
        assert!(!directory
            .path()
            .join(format!(".now.{}.tmp", std::process::id()))
            .exists());
    }
}
