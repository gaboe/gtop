use serde::Serialize;
use std::{
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const INTERVAL: Duration = Duration::from_secs(2);
const PROBE_INTERVAL: Duration = Duration::from_secs(15);
const PROBE_TIMEOUT: Duration = Duration::from_secs(4);
const PROBE_WINDOW: Duration = Duration::from_secs(5 * 60);
const PROBE_LIMIT: usize = 20;
const LATENCY_PROBES: usize = 4;
const TARGET: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(1, 1, 1, 1), 443));

#[derive(Clone, Copy, Debug)]
pub(crate) struct SampledMetrics {
    pub(crate) time: u64,
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

#[derive(Default)]
struct ProbeReading {
    recent: Vec<(u64, Outcome)>,
}

impl ProbeReading {
    fn record(&mut self, at: u64, outcome: Outcome) {
        self.recent.push((at, outcome));
        self.expire(at);
    }

    fn expire(&mut self, now: u64) {
        self.recent
            .retain(|(at, _)| *at <= now && now - *at <= PROBE_WINDOW.as_secs());
        if self.recent.len() > PROBE_LIMIT {
            self.recent.drain(..self.recent.len() - PROBE_LIMIT);
        }
    }

    fn latency_ms(&self) -> Option<f64> {
        let mut successful: Vec<_> = self
            .recent
            .iter()
            .rev()
            .filter_map(|(_, outcome)| match outcome {
                Outcome::Reply(duration) => Some(duration.as_secs_f64() * 1000.),
                Outcome::Lost => None,
            })
            .take(LATENCY_PROBES)
            .collect();
        successful.sort_by(f64::total_cmp);
        successful.get(successful.len() / 2).copied()
    }

    fn loss_percent(&self) -> Option<f64> {
        (!self.recent.is_empty()).then(|| {
            self.recent
                .iter()
                .filter(|(_, outcome)| *outcome == Outcome::Lost)
                .count() as f64
                / self.recent.len() as f64
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

fn snapshot(metrics: SampledMetrics, reading: &ProbeReading) -> NowSnapshot {
    NowSnapshot {
        version: 1,
        time: metrics.time,
        interval_s: 2,
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

fn write_atomic(directory: &Path, value: &NowSnapshot) -> io::Result<()> {
    std::fs::create_dir_all(directory)?;
    let file = directory.join("now.json");
    let temporary = directory.join(format!(".now.{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(temporary, file)
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
    metrics: Arc<Mutex<Option<SampledMetrics>>>,
    stop: Arc<StopSignal>,
    _workers: Vec<JoinHandle<()>>,
}

impl Writer {
    pub(crate) fn start(directory: PathBuf) -> io::Result<Self> {
        let metrics = Arc::new(Mutex::new(None));
        let reading = Arc::new(Mutex::new(ProbeReading::default()));
        let stop = Arc::new(StopSignal::new());

        let probe_stop = stop.clone();
        let probe_reading = reading.clone();
        let probe = thread::Builder::new()
            .name("gtop-network-probe".into())
            .spawn(move || loop {
                let due = Instant::now() + PROBE_INTERVAL;
                let outcome = probe_once(connect);
                let mut reading = probe_reading
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                reading.record(unix_seconds(), outcome);
                drop(reading);
                if !probe_stop.wait(due.saturating_duration_since(Instant::now())) {
                    break;
                }
            })?;

        let writer_stop = stop.clone();
        let writer_metrics = metrics.clone();
        let writer_reading = reading;
        let writer = match thread::Builder::new()
            .name("gtop-now-json".into())
            .spawn(move || {
                while writer_stop.wait(INTERVAL) {
                    let metrics = *writer_metrics
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    let Some(metrics) = metrics else { continue };
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
            metrics,
            stop,
            _workers: vec![probe, writer],
        })
    }

    pub(crate) fn update(&self, metrics: SampledMetrics) {
        *self
            .metrics
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(metrics);
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
        let value = snapshot(metrics(), &reading);
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
        reading.expire(1_306);
        assert_eq!(reading.loss_percent(), None);
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
        assert_eq!(reading.recent.len(), PROBE_LIMIT);
        assert_eq!(reading.latency_ms(), Some(23.));
    }
}
