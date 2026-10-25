#[cfg(test)]
#[path = "../../../crates/gtop/src/test_data.rs"]
mod test_data;
#[cfg(test)]
mod rack {
    pub fn now() -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
    }
}
#[allow(dead_code)]
#[path = "../../../crates/gtop/src/analysis.rs"]
mod analysis;
mod baseline;
#[allow(dead_code)]
#[path = "../../../crates/gtop/src/history.rs"]
mod history;
use serde_json::json;
use std::{hint::black_box, path::Path, time::Instant};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
fn cpu() -> f64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // getrusage initializes the complete struct on success.
    assert_eq!(
        unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) },
        0
    );
    let usage = unsafe { usage.assume_init() };
    usage.ru_utime.tv_sec as f64
        + usage.ru_utime.tv_usec as f64 / 1e6
        + usage.ru_stime.tv_sec as f64
        + usage.ru_stime.tv_usec as f64 / 1e6
}
fn timed<T>(operation: &str, f: impl FnOnce() -> T) -> T {
    let c = cpu();
    let start = Instant::now();
    let result = black_box(f());
    println!(
        "{}",
        json!({"operation":operation,"cpu_ms":(cpu()-c)*1000.,"wall_ms":start.elapsed().as_secs_f64()*1000.})
    );
    result
}
fn check(a: &history::HistoryView, b: &history::HistoryView) {
    assert_eq!(a.covered, b.covered);
    assert_eq!(a.start_minute, b.start_minute);
    assert_eq!(a.columns.len(), b.columns.len());
    let near = |a: f64, b: f64| {
        assert!(
            (a - b).abs() <= 1e-8 * a.abs().max(b.abs()).max(1.),
            "{a} != {b}"
        )
    };
    near(a.total_cpu, b.total_cpu);
    assert_eq!(a.rows.len(), b.rows.len());
    let rows: std::collections::BTreeMap<_, _> = b.rows.iter().map(|(n, p)| (n, p)).collect();
    for (name, p) in &a.rows {
        let q = rows[name];
        near(p.cpu_seconds, q.cpu_seconds);
        near(p.gpu_seconds, q.gpu_seconds);
        assert_eq!(p.peak_memory, q.peak_memory);
        assert_eq!(p.peak_cpu, q.peak_cpu);
    }
    for (a, b) in a.columns.iter().zip(&b.columns) {
        assert_eq!(a.is_some(), b.is_some());
        if let (Some(a), Some(b)) = (a, b) {
            near(a.seconds, b.seconds);
            near(a.cpu, b.cpu);
            near(a.memory, b.memory);
            assert_eq!(a.gpu, b.gpu);
            assert_eq!(a.temperature, b.temperature);
        }
    }
    for metric in history::Metric::ALL {
        let a = a.leaders(metric);
        let b = b.leaders(metric);
        assert_eq!(a.len(), b.len());
        for (a, b) in a.iter().zip(&b) {
            assert_eq!(a.0, b.0);
            near(a.1, b.1);
        }
    }
}
fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 5 {
        return Err("Usage: gtop-history-bench prepare|json|sqlite input.json scratch-directory anchor-minute".into());
    }
    let mode = &args[1];
    let input = Path::new(&args[2]);
    let directory = Path::new(&args[3]);
    let minute: u64 = args[4].parse()?;
    let now = minute * 60;
    if mode == "replay-sparse" || mode == "replay-dense" {
        let days = minute;
        let source: baseline::History = serde_json::from_reader(std::fs::File::open(input)?)?;
        let dense = mode == "replay-dense";
        let path = directory.join("history.sqlite");
        std::fs::create_dir_all(directory)?;
        let anchor = if path.exists() {
            let connection = rusqlite::Connection::open(&path)?;
            connection.query_row(
                "SELECT MAX(minute) FROM system WHERE resolution=1",
                [],
                |r| r.get::<_, i64>(0),
            )? as u64
                + 1
        } else {
            100800
        };
        let mut history =
            history::History::open(&path, &directory.join("absent.json"), anchor * 60)?;
        let first = source.buckets.front().ok_or("Empty replay fixture")?;
        let mut commits = 0;
        for day in 0..days {
            if dense {
                for offset in 0..1440 {
                    let mut bucket = first.clone();
                    bucket.minute = anchor + day * 1440 + offset;
                    bucket.seconds = 60.;
                    let now = bucket.minute * 60;
                    history.benchmark_minute(bucket);
                    history.persist(now)?;
                    commits += 1;
                }
            } else {
                for original in &source.buckets {
                    let mut bucket = original.clone();
                    bucket.minute = anchor + day * 1440 + original.minute - first.minute;
                    let now = bucket.minute * 60;
                    history.benchmark_minute(bucket);
                    history.persist(now)?;
                    commits += 1;
                }
            }
            println!(
                "{}",
                json!({"day":day+1,"commits":commits,"bytes":std::fs::metadata(&path)?.len()})
            );
        }
        drop(history);
        let connection = rusqlite::Connection::open(&path)?;
        let integrity: String = connection.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        assert_eq!(integrity, "ok");
        let rows: i64 =
            connection.query_row("SELECT COUNT(*) FROM system WHERE resolution=1", [], |r| {
                r.get(0)
            })?;
        assert!(rows > 0);
        println!(
            "{}",
            json!({"verified":true,"commits":commits,"integrity":integrity,"bytes":std::fs::metadata(&path)?.len()})
        );
        return Ok(());
    }
    if mode == "prepare" {
        std::fs::create_dir_all(directory)?;
        let store = history::History::open(&directory.join("history.sqlite"), input, now)?;
        let baseline = baseline::History::load(input, now);
        for hours in [1, 8, 24] {
            check(&store.view(hours, now)?, &baseline.view(hours, now));
        }
        println!(
            "{}",
            json!({"prepared":true,"minutes":baseline.buckets.len(),"process_entries":baseline.buckets.iter().map(|b|b.processes.len()).sum::<usize>()})
        );
        return Ok(());
    }
    if mode == "retain" {
        let history = history::History::open(&directory.join("history.sqlite"), input, now)?;
        let view = history.view(24, now)?;
        println!(
            "{}",
            json!({"retained":true,"covered":view.covered,"rows":view.rows.len()})
        );
        return Ok(());
    }
    if mode == "profile-json" || mode == "profile-sqlite" {
        use std::io::Write;
        let json_history = (mode == "profile-json").then(|| baseline::History::load(input, now));
        let sqlite_history = if mode == "profile-sqlite" {
            Some(history::History::open(
                &directory.join("history.sqlite"),
                input,
                now,
            )?)
        } else {
            None
        };
        println!("{}", json!({"profile_ready":true,"pid":std::process::id()}));
        std::io::stdout().flush()?;
        let start = Instant::now();
        let c = cpu();
        let mut views = 0;
        while start.elapsed().as_secs() < 12 {
            let view = if let Some(history) = &json_history {
                history.view(24, now)
            } else {
                sqlite_history.as_ref().unwrap().view(24, now)?
            };
            assert!(view.covered > 0);
            black_box(view.rows.len());
            views += 1;
        }
        println!(
            "{}",
            json!({"profile_complete":true,"views":views,"cpu_seconds":cpu()-c,"wall_seconds":start.elapsed().as_secs_f64()})
        );
        return Ok(());
    }
    if mode == "json" {
        let mut history = timed("startup", || {
            let history = baseline::History::load(input, now);
            black_box([1, 8, 24].map(|h| history.view(h, now)));
            history
        });
        for h in [1, 24] {
            let view = timed(&format!("view_{h}h"), || history.view(h, now));
            assert!(view.covered > 0);
            black_box(view.rows.len());
        }
        let mut bucket = history
            .buckets
            .iter()
            .max_by_key(|b| b.processes.len())
            .unwrap()
            .clone();
        bucket.minute = minute + 1;
        history.buckets.push_back(bucket);
        history
            .buckets
            .retain(|b| b.minute > minute.saturating_sub(1438));
        timed("append_minute", || {
            history.save(&directory.join("append.json"))
        })?;
        let reloaded = baseline::History::load(&directory.join("append.json"), (minute + 1) * 60);
        assert_eq!(reloaded.buckets.back().unwrap().minute, minute + 1);
        println!(
            "{}",
            json!({"verified":true,"bytes":std::fs::metadata(directory.join("append.json"))?.len()})
        );
    } else if mode == "sqlite" {
        let mut history = timed("startup", || -> Result<history::History> {
            let history = history::History::open(&directory.join("history.sqlite"), input, now)?;
            black_box([1, 8, 24].map(|h| history.view(h, now).unwrap()));
            Ok(history)
        })?;
        for h in [1, 24] {
            let view = timed(&format!("view_{h}h"), || history.view(h, now).unwrap());
            assert!(view.covered > 0);
            black_box(view.rows.len());
        }
        let source = baseline::History::load(input, now);
        let mut bucket = source
            .buckets
            .iter()
            .max_by_key(|b| b.processes.len())
            .unwrap()
            .clone();
        bucket.minute = minute + 1;
        history.benchmark_minute(bucket);
        timed("append_minute", || history.persist((minute + 1) * 60))?;
        drop(history);
        let reloaded =
            history::History::open(&directory.join("history.sqlite"), input, (minute + 1) * 60)?;
        let view = reloaded.view(1, (minute + 1) * 60)?;
        assert!(view.columns.last().unwrap().is_some());
        println!("{}", json!({"verified":true,"rows":view.rows.len()}));
    } else {
        return Err("Unknown benchmark mode".into());
    }
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
