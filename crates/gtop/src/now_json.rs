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
    pub(crate) cpu_percent: f64,
    pub(crate) memory_used_bytes: u64,
    pub(crate) memory_total_bytes: u64,
    pub(crate) disk_free_bytes: Option<u64>,
    pub(crate) disk_total_bytes: Option<u64>,
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
        if self.loss.len() > PROBE_LIMIT {
            self.loss.drain(..self.loss.len() - PROBE_LIMIT);
        }
        self.latency
            .retain(|(at, _)| *at <= now && now - *at <= PROBE_WINDOW.as_secs());
        if self.latency.len() > LATENCY_PROBES {
            self.latency.drain(..self.latency.len() - LATENCY_PROBES);
        }
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

fn snapshot_at(metrics: SampledMetrics, reading: &ProbeReading, now: u64) -> NowSnapshot {
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
            read_bytes_per_s: None,
            write_bytes_per_s: None,
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

fn snapshot(metrics: SampledMetrics, reading: &ProbeReading) -> NowSnapshot {
    snapshot_at(metrics, reading, unix_seconds())
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
    connector: impl Fn(SocketAddr, Duration) -> io::Result<Duration>,
) {
    loop {
        if !stop.wait(Duration::ZERO) {
            break;
        }
        let due = Instant::now() + PROBE_INTERVAL;
        let outcome = probe_once(&connector);
        reading
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .record(unix_seconds(), outcome);
        if !stop.wait(due.saturating_duration_since(Instant::now())) {
            break;
        }
    }
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
        let (updates, receiver) = mpsc::channel();
        let reading = Arc::new(Mutex::new(ProbeReading::default()));
        let stop = Arc::new(StopSignal::new());

        let probe_stop = stop.clone();
        let probe_reading = reading.clone();
        let probe = thread::Builder::new()
            .name("gtop-network-probe".into())
            .spawn(move || probe_loop(probe_stop, probe_reading, connect))?;

        let writer_stop = stop.clone();
        let writer_reading = reading;
        let writer = match thread::Builder::new()
            .name("gtop-now-json".into())
            .spawn(move || {
                while writer_stop.wait(Duration::ZERO) {
                    let metrics = match receiver.recv() {
                        Ok(metrics) => metrics,
                        Err(_) => break,
                    };
                    let reading = writer_reading
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    if let Err(error) = write_atomic(&directory, &snapshot(metrics, &reading)) {
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
    use std::sync::atomic::{AtomicBool, Ordering};

    fn metrics() -> SampledMetrics {
        SampledMetrics {
            time: 1_800_000_000,
            interval_s: 2,
            cpu_percent: 12.5,
            memory_used_bytes: 500,
            memory_total_bytes: 1000,
            disk_free_bytes: Some(2000),
            disk_total_bytes: Some(4000),
            rx_bytes_per_s: 10,
            tx_bytes_per_s: 20,
            temperature_c: None,
        }
    }

    #[test]
    fn serialization_matches_the_version_one_format() {
        let mut reading = ProbeReading::default();
        reading.record(1_800_000_000, Outcome::Reply(Duration::from_millis(25)));
        let value = snapshot_at(metrics(), &reading, 1_800_000_000);
        let json = serde_json::to_string(&value).unwrap();
        let expected = format!(
            r#"{{"version":1,"time":1800000000,"interval_s":2,"pid":{},"commit":"{}","cpu":{{"percent":12.5}},"memory":{{"used_bytes":500,"total_bytes":1000,"percent":50.0}},"ssd":{{"free_bytes":2000,"total_bytes":4000,"read_bytes_per_s":null,"write_bytes_per_s":null,"temperature_c":null}},"net":{{"rx_bytes_per_s":10,"tx_bytes_per_s":20,"latency_ms":25.0,"loss_percent":0.0,"probe":"tcp 1.1.1.1:443"}}}}"#,
            std::process::id(),
            option_env!("GTOP_GIT_COMMIT").unwrap_or("unknown")
        );
        assert_eq!(json, expected);
        println!("SAMPLE_NOW_JSON={json}");
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
    fn publication_expires_old_probe_results() {
        let mut reading = ProbeReading::default();
        reading.record(1_000, Outcome::Reply(Duration::from_millis(25)));
        reading.record(1_000, Outcome::Lost);
        let value = snapshot_at(metrics(), &reading, 1_301);
        let json = serde_json::to_value(value).unwrap();
        assert_eq!(json["net"]["latency_ms"], serde_json::Value::Null);
        assert_eq!(json["net"]["loss_percent"], serde_json::Value::Null);
    }

    #[test]
    fn stopped_probe_does_not_connect() {
        let stop = Arc::new(StopSignal::new());
        stop.stop();
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let connector_called = called.clone();
        probe_loop(
            stop,
            Arc::new(Mutex::new(ProbeReading::default())),
            move |_, _| {
                connector_called.store(true, Ordering::Relaxed);
                Ok(Duration::ZERO)
            },
        );
        assert!(!called.load(Ordering::Relaxed));
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
