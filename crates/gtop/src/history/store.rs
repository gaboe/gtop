use super::{Bucket, History, ProcessTotal};
use rusqlite::{params, Connection, Row, Transaction, TransactionBehavior};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::File,
    ops::Range,
    path::Path,
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
struct SqlU64(u64);
impl rusqlite::types::ToSql for SqlU64 {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        i64::try_from(self.0)
            .map(rusqlite::types::ToSqlOutput::from)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
    }
}
impl rusqlite::types::FromSql for SqlU64 {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        let value = <i64 as rusqlite::types::FromSql>::column_result(value)?;
        u64::try_from(value)
            .map(Self)
            .map_err(|_| rusqlite::types::FromSqlError::OutOfRange(value))
    }
}
fn unsigned(row: &Row<'_>, index: usize) -> rusqlite::Result<u64> {
    Ok(row.get::<_, SqlU64>(index)?.0)
}

type Totals = BTreeMap<String, ProcessTotal>;

pub struct Store {
    connection: Connection,
    _lock: File,
}

impl Store {
    pub fn open(path: &Path, legacy: &Path, minute: u64) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_extension("lock"))?;
        lock.try_lock()
            .map_err(|error| format!("History is open in another gtop instance: {error}"))?;
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version != 0 && version != 1 {
            return Err(format!("Unsupported history database version {version}").into());
        }
        connection.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA cache_size=-16384;",
        )?;
        connection.execute_batch(include_str!("schema.sql"))?;
        let migrated: bool =
            connection.query_row("SELECT EXISTS(SELECT 1 FROM migration)", [], |r| r.get(0))?;
        if !migrated {
            let buckets = if legacy.exists() {
                let metadata = std::fs::metadata(legacy)?;
                if metadata.len() >= 512 * 1024 * 1024 {
                    return Err("Legacy history exceeds the import size limit".into());
                }
                let mut history: History =
                    serde_json::from_reader(std::io::BufReader::new(File::open(legacy)?))?;
                for bucket in &mut history.buckets {
                    if bucket.peak_cpu == 0. {
                        bucket.peak_cpu = bucket.cpu;
                    }
                    if bucket.peak_memory == 0. {
                        bucket.peak_memory = bucket.memory;
                    }
                    bucket.peak_gpu = bucket.peak_gpu.or(bucket.gpu);
                }
                history.buckets
            } else {
                VecDeque::new()
            };
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            for bucket in &buckets {
                write_bucket(&tx, bucket)?;
            }
            tx.commit()?;
            // Reopen separately before accepting the imported store. The source stays untouched.
            let verify = Connection::open(path)?;
            let count: i64 =
                verify.query_row("SELECT COUNT(*) FROM system WHERE resolution=1", [], |r| {
                    r.get(0)
                })?;
            let expected_processes: usize = buckets.iter().map(|b| b.processes.len()).sum();
            let expected_cpu: f64 = buckets
                .iter()
                .flat_map(|b| b.processes.values())
                .map(|p| p.cpu_seconds)
                .sum();
            let expected_gpu: f64 = buckets
                .iter()
                .flat_map(|b| b.processes.values())
                .map(|p| p.gpu_seconds)
                .sum();
            let expected_seconds: f64 = buckets.iter().map(|b| b.seconds).sum();
            let (processes,cpu,gpu): (i64,f64,f64) = verify.query_row("SELECT COUNT(*),COALESCE(SUM(cpu_seconds),0),COALESCE(SUM(gpu_seconds),0) FROM process WHERE resolution=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            let seconds: f64 = verify.query_row(
                "SELECT COALESCE(SUM(seconds),0) FROM system WHERE resolution=1",
                [],
                |r| r.get(0),
            )?;
            let near = |a: f64, b: f64| (a - b).abs() <= 1e-8 * a.abs().max(b.abs()).max(1.);
            if count != buckets.len() as i64
                || processes != expected_processes as i64
                || !near(cpu, expected_cpu)
                || !near(gpu, expected_gpu)
                || !near(seconds, expected_seconds)
            {
                return Err("History import verification failed".into());
            }
            drop(verify);
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("INSERT INTO migration VALUES(1,?1)", [buckets.len() as i64])?;
            tx.commit()?;
        }
        let mut store = Self {
            connection,
            _lock: lock,
        };
        store.append(std::iter::empty(), minute)?;
        Ok(store)
    }

    pub fn append<'a>(
        &mut self,
        buckets: impl IntoIterator<Item = &'a Bucket>,
        minute: u64,
    ) -> Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let watermark: u64 =
            tx.query_row("SELECT compacted_before FROM meta", [], |r| unsigned(r, 0))?;
        for bucket in buckets {
            if bucket.minute < watermark {
                return Err("History minute has already been compacted".into());
            }
            write_bucket(&tx, bucket)?;
        }
        compact(&tx, minute)?;
        tx.commit()?;
        Ok(())
    }

    fn system(&self, range: Range<u64>) -> Result<VecDeque<Bucket>> {
        let mut statement = self.connection.prepare_cached("SELECT minute,seconds,cpu,memory,gpu,gpu_seconds,temperature,temp_seconds,peak_cpu,peak_memory,peak_gpu,peak_temp FROM system WHERE resolution=1 AND minute>=?1 AND minute<?2 ORDER BY minute")?;
        let buckets = statement
            .query_map(
                params![SqlU64(range.start), SqlU64(range.end)],
                system_bucket,
            )?
            .collect::<rusqlite::Result<_>>()?;
        Ok(buckets)
    }

    pub fn buckets(&self, range: Range<u64>, name: Option<&str>) -> Result<VecDeque<Bucket>> {
        let mut buckets = self.system(range.clone())?;
        let indexes: BTreeMap<_, _> = buckets
            .iter()
            .enumerate()
            .map(|(i, b)| (b.minute, i))
            .collect();
        let sql = if name.is_some() {
            "SELECT p.minute,n.name,p.cpu_seconds,p.gpu_seconds,p.peak_memory,p.peak_cpu FROM names n JOIN process p INDEXED BY process_name_minute ON p.name_id=n.id WHERE n.name=?3 AND p.resolution=1 AND p.minute>=?1 AND p.minute<?2"
        } else {
            "SELECT p.minute,n.name,p.cpu_seconds,p.gpu_seconds,p.peak_memory,p.peak_cpu FROM process p INDEXED BY process_minute JOIN names n ON n.id=p.name_id WHERE p.resolution=1 AND p.minute>=?1 AND p.minute<?2 AND ?3 IS NULL"
        };
        let mut statement = self.connection.prepare_cached(sql)?;
        let mut rows = statement.query(params![SqlU64(range.start), SqlU64(range.end), name])?;
        while let Some(row) = rows.next()? {
            let minute: u64 = unsigned(row, 0)?;
            if let Some(index) = indexes.get(&minute) {
                buckets[*index]
                    .processes
                    .insert(row.get(1)?, process_total(row, 2)?);
            }
        }
        Ok(buckets)
    }

    pub fn view_data(
        &self,
        range: Range<u64>,
        live: &VecDeque<Bucket>,
    ) -> Result<(VecDeque<Bucket>, Totals)> {
        let excluded = serde_json::to_string(&live.iter().map(|b| b.minute).collect::<Vec<_>>())?;
        let mut statement = self.connection.prepare_cached("SELECT n.name,SUM(p.cpu_seconds),SUM(p.gpu_seconds),MAX(p.peak_memory),MAX(p.peak_cpu) FROM process p INDEXED BY process_minute JOIN names n ON n.id=p.name_id WHERE p.resolution=1 AND p.minute>=?1 AND p.minute<?2 AND p.minute NOT IN (SELECT value FROM json_each(?3)) GROUP BY p.name_id")?;
        let mut rows =
            statement.query(params![SqlU64(range.start), SqlU64(range.end), excluded])?;
        let mut totals = BTreeMap::new();
        while let Some(row) = rows.next()? {
            totals.insert(row.get(0)?, process_total(row, 1)?);
        }
        Ok((self.system(range)?, totals))
    }
}

fn system_bucket(row: &Row<'_>) -> rusqlite::Result<Bucket> {
    Ok(Bucket {
        minute: unsigned(row, 0)?,
        seconds: row.get(1)?,
        cpu: row.get(2)?,
        memory: row.get(3)?,
        gpu: row.get(4)?,
        gpu_seconds: Some(row.get(5)?),
        temperature: row.get(6)?,
        temperature_seconds: Some(row.get(7)?),
        peak_cpu: row.get(8)?,
        peak_memory: row.get(9)?,
        peak_gpu: row.get(10)?,
        peak_temperature: row.get(11)?,
        ..Default::default()
    })
}
fn process_total(row: &Row<'_>, offset: usize) -> rusqlite::Result<ProcessTotal> {
    Ok(ProcessTotal {
        cpu_seconds: row.get(offset)?,
        gpu_seconds: row.get(offset + 1)?,
        peak_memory: unsigned(row, offset + 2)?,
        peak_cpu: row.get(offset + 3)?,
    })
}

fn write_bucket(tx: &Transaction<'_>, b: &Bucket) -> Result<()> {
    tx.execute(
        "DELETE FROM process WHERE resolution=1 AND minute=?1",
        [SqlU64(b.minute)],
    )?;
    tx.execute(
        "INSERT OR REPLACE INTO system VALUES(1,?1,1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![
            SqlU64(b.minute),
            b.seconds,
            b.cpu,
            b.memory,
            b.gpu,
            b.gpu.map_or(0., |_| b.gpu_seconds.unwrap_or(b.seconds)),
            b.temperature,
            b.temperature
                .map_or(0., |_| b.temperature_seconds.unwrap_or(b.seconds)),
            b.peak_cpu,
            b.peak_memory,
            b.peak_gpu,
            b.peak_temperature
        ],
    )?;
    let mut intern = tx.prepare_cached("INSERT OR IGNORE INTO names(name) VALUES(?1)")?;
    let mut insert = tx.prepare_cached(
        "INSERT INTO process SELECT 1,?1,id,?3,?4,?5,?6 FROM names WHERE name=?2",
    )?;
    for (name, p) in &b.processes {
        intern.execute([name])?;
        insert.execute(params![
            SqlU64(b.minute),
            name,
            p.cpu_seconds,
            p.gpu_seconds,
            SqlU64(p.peak_memory),
            p.peak_cpu
        ])?;
    }
    Ok(())
}

fn compact(tx: &Transaction<'_>, now: u64) -> Result<()> {
    let cutoff = now.saturating_sub(7 * 1440) / 10 * 10;
    let expired = now.saturating_sub(30 * 1440) / 10 * 10;
    let watermark: u64 =
        tx.query_row("SELECT compacted_before FROM meta", [], |r| unsigned(r, 0))?;
    if cutoff <= watermark {
        return Ok(());
    }
    tx.execute("INSERT OR REPLACE INTO system SELECT 10,minute/10*10,SUM(recorded),SUM(seconds),COALESCE(SUM(cpu*seconds)/NULLIF(SUM(seconds),0),MAX(cpu)),COALESCE(SUM(memory*seconds)/NULLIF(SUM(seconds),0),MAX(memory)),COALESCE(SUM(gpu*gpu_seconds)/NULLIF(SUM(gpu_seconds),0),MAX(gpu)),SUM(gpu_seconds),COALESCE(SUM(temperature*temp_seconds)/NULLIF(SUM(temp_seconds),0),MAX(temperature)),SUM(temp_seconds),MAX(peak_cpu),MAX(peak_memory),MAX(peak_gpu),MAX(peak_temp) FROM system WHERE resolution=1 AND minute<?1 GROUP BY minute/10",[SqlU64(cutoff)])?;
    tx.execute("INSERT OR REPLACE INTO process SELECT 10,minute/10*10,name_id,SUM(cpu_seconds),SUM(gpu_seconds),MAX(peak_memory),MAX(peak_cpu) FROM process WHERE resolution=1 AND minute<?1 GROUP BY minute/10,name_id",[SqlU64(cutoff)])?;
    for table in ["process", "system"] {
        tx.execute(&format!("DELETE FROM {table} WHERE (resolution=1 AND minute<?1) OR (resolution=10 AND minute<?2)"),params![SqlU64(cutoff),SqlU64(expired)])?;
    }
    tx.execute(
        "DELETE FROM names WHERE NOT EXISTS(SELECT 1 FROM process WHERE process.name_id=names.id)",
        [],
    )?;
    tx.execute("UPDATE meta SET compacted_before=?1", [SqlU64(cutoff)])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "gtop-sqlite-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn open(&self, minute: u64) -> Store {
            Store::open(
                &self.0.join("history.sqlite"),
                &self.0.join("history.json"),
                minute,
            )
            .unwrap()
        }
        fn history(&self, minute: u64) -> History {
            History::open(
                &self.0.join("history.sqlite"),
                &self.0.join("history.json"),
                minute * 60,
            )
            .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn bucket(minute: u64, seconds: f64, cpu: f64) -> Bucket {
        Bucket {
            minute,
            seconds,
            cpu,
            peak_cpu: cpu + 10.,
            memory: cpu * 10.,
            peak_memory: cpu * 20.,
            processes: BTreeMap::from([
                (
                    "worker".into(),
                    ProcessTotal {
                        cpu_seconds: cpu,
                        gpu_seconds: 2.,
                        peak_memory: 100,
                        peak_cpu: cpu as f32,
                    },
                ),
                ("zero".into(), ProcessTotal::default()),
            ]),
            ..Default::default()
        }
    }
    #[test]
    fn append_replaces_atomically_and_keeps_zero_process_presence() {
        let f = Fixture::new();
        let mut s = f.open(101);
        let b = bucket(100, 20., 30.);
        s.append([&b], 101).unwrap();
        s.append([&b], 101).unwrap();
        let loaded = s.buckets(100..102, None).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].processes.len(), 2);
        assert_eq!(loaded[0].processes["worker"].cpu_seconds, 30.);
        let (system, rows) = s.view_data(100..102, &VecDeque::new()).unwrap();
        assert_eq!(system[0].seconds, 20.);
        assert_eq!(rows["worker"].cpu_seconds, 30.);
        let named = s.buckets(100..102, Some("zero")).unwrap();
        assert!(crate::analysis::analyze("zero", "zero", 0., 101, &named)
            .history
            .is_some());
    }
    #[test]
    fn compaction_weights_each_sensor_preserves_peaks_and_expires() {
        let f = Fixture::new();
        let mut s = f.open(101);
        let mut a = bucket(100, 10., 20.);
        a.gpu = Some(80.);
        a.gpu_seconds = Some(2.);
        a.peak_gpu = Some(90.);
        a.temperature = Some(50.);
        a.temperature_seconds = Some(0.);
        a.peak_temperature = Some(55.);
        let mut b = bucket(101, 30., 60.);
        b.gpu = Some(20.);
        b.gpu_seconds = Some(6.);
        b.peak_gpu = Some(30.);
        s.append([&a, &b], 101).unwrap();
        s.append(std::iter::empty(), 7 * 1440 + 110).unwrap();
        let values:(i64,f64,f64,f64,f64,f64,f64,f64)=s.connection.query_row("SELECT recorded,seconds,cpu,gpu,temperature,peak_cpu,peak_gpu,peak_temp FROM system WHERE resolution=10 AND minute=100",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).unwrap();
        assert_eq!(values, (2, 40., 50., 35., 50., 70., 90., 55.));
        let total:f64=s.connection.query_row("SELECT cpu_seconds FROM process WHERE resolution=10 AND name_id=(SELECT id FROM names WHERE name='worker')",[],|r|r.get(0)).unwrap();
        assert_eq!(total, 80.);
        s.append(std::iter::empty(), 101).unwrap();
        assert!(s.append([&a], 101).is_err());
        s.append(std::iter::empty(), 30 * 1440 + 110).unwrap();
        for table in ["system", "process", "names"] {
            assert_eq!(
                s.connection
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
    }
    #[test]
    fn migration_verifies_totals_and_never_changes_json() {
        let f = Fixture::new();
        let mut legacy = History::default();
        legacy.buckets.push_back(bucket(100, 30., 42.));
        let json = serde_json::to_vec(&legacy).unwrap();
        std::fs::write(f.0.join("history.json"), &json).unwrap();
        {
            let s = f.open(100);
            assert_eq!(s.buckets(100..101, None).unwrap()[0].processes.len(), 2);
        }
        assert_eq!(std::fs::read(f.0.join("history.json")).unwrap(), json);
        std::fs::write(f.0.join("history.json"), b"invalid after import").unwrap();
        let s = f.open(100);
        assert_eq!(
            s.connection
                .query_row("SELECT imported FROM migration", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(s.buckets(100..101, None).unwrap()[0].cpu, 42.);
    }
    #[test]
    fn lifetime_lock_rejects_a_second_stale_writer() {
        let f = Fixture::new();
        let mut first = f.history(100);
        assert!(
            History::open(&f.0.join("history.sqlite"), &f.0.join("history.json"), 6000).is_err()
        );
        first.buckets.push_back(bucket(100, 20., 42.));
        first.dirty.insert(100);
        first.persist(6000).unwrap();
        drop(first);
        let second = f.history(100);
        assert_eq!(second.buckets[0].cpu, 42.);
    }
    #[test]
    fn failed_recovery_read_does_not_mutate_or_dirty_the_sample() {
        let f = Fixture::new();
        let mut h = f.history(100);
        let snapshot = crate::test_data::snapshot();
        h.record(&snapshot, 6054., 3.);
        h.record(&snapshot, 6057., 3.);
        h.persist(6057).unwrap();
        let before = serde_json::to_vec(&h).unwrap();
        let last = h.last_sample;
        h.store
            .as_ref()
            .unwrap()
            .connection
            .execute_batch("ALTER TABLE process RENAME TO unavailable_process")
            .unwrap();
        assert!(!h.record(&snapshot, 6060., 3.));
        assert_eq!(serde_json::to_vec(&h).unwrap(), before);
        assert_eq!(h.last_sample, last);
        assert!(h.dirty.is_empty());
        assert!(h.take_error().is_some());
        h.store
            .as_ref()
            .unwrap()
            .connection
            .execute_batch("ALTER TABLE unavailable_process RENAME TO process")
            .unwrap();
        h.record(&snapshot, 6060., 3.);
        h.persist(6060).unwrap();
        let loaded = h.store.as_ref().unwrap().buckets(100..102, None).unwrap();
        assert_eq!(
            loaded
                .iter()
                .map(|b| b.processes["rustc"].cpu_seconds)
                .sum::<f64>(),
            21.
        );
    }
    #[test]
    fn startup_keeps_only_current_minute_and_recovery_persists_older_minutes() {
        let f = Fixture::new();
        {
            let mut s = f.open(101);
            s.append([&bucket(100, 10., 20.)], 101).unwrap();
        }
        let mut h = f.history(101);
        assert!(h.buckets.is_empty());
        let mut snapshot = crate::test_data::snapshot();
        h.record(&snapshot, 6060., 3.);
        h.record(&snapshot, 6063., 3.);
        h.persist(6063).unwrap();
        snapshot.process_data = gtop_core::ProcessData::Stale {
            since: std::time::Instant::now(),
            reason: "probe".into(),
        };
        h.record(&snapshot, 6120., 60.);
        h.persist(6120).unwrap();
        assert!(h.buckets.iter().all(|b| b.minute >= 102));
        snapshot.process_data = gtop_core::ProcessData::Fresh;
        for p in std::sync::Arc::make_mut(&mut snapshot.processes) {
            p.cpu_sample_seconds = Some(60.);
        }
        h.record(&snapshot, 6123., 60.);
        assert!(h.dirty.contains(&101));
        h.persist(6123).unwrap();
        let persisted = h.store.as_ref().unwrap().buckets(101..102, None).unwrap();
        assert!(persisted[0].processes["rustc"].cpu_seconds > 10.5);
        assert_eq!(h.view(1, 6123).covered, 3);
    }
    #[test]
    fn minute_reads_use_bounded_index_searches() {
        let f = Fixture::new();
        let s = f.open(100);
        let sql="EXPLAIN QUERY PLAN SELECT name_id,SUM(cpu_seconds) FROM process INDEXED BY process_minute WHERE resolution=1 AND minute>=50 AND minute<100 GROUP BY name_id";
        let plans: Vec<String> = s
            .connection
            .prepare(sql)
            .unwrap()
            .query_map([], |r| r.get(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(plans.iter().any(|p|p.contains("SEARCH process USING INDEX process_minute (resolution=? AND minute>? AND minute<?)")),"{plans:?}");
        let indexes: i64 = s
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE type='index' AND tbl_name='process'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(indexes, 2);
    }
    #[test]
    #[ignore]
    fn crash_child() {
        let Some(path) = std::env::var_os("GTOP_HISTORY_CRASH_DIR") else {
            return;
        };
        let path = std::path::PathBuf::from(path);
        let mut s = Store::open(
            &path.join("history.sqlite"),
            &path.join("history.json"),
            101,
        )
        .unwrap();
        let b = bucket(101, 30., 60.);
        if std::env::var("GTOP_HISTORY_CRASH_PHASE").unwrap() == "before" {
            let tx = s
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            write_bucket(&tx, &b).unwrap();
            std::fs::write(path.join("ready"), b"before").unwrap();
            loop {
                std::thread::park_timeout(std::time::Duration::from_secs(1));
            }
        } else {
            s.append([&b], 101).unwrap();
            std::fs::write(path.join("ready"), b"after").unwrap();
            loop {
                std::thread::park_timeout(std::time::Duration::from_secs(1));
            }
        }
    }
    #[test]
    fn killed_writer_keeps_all_committed_minutes() {
        for phase in ["before", "after"] {
            let f = Fixture::new();
            {
                let mut s = f.open(101);
                s.append([&bucket(100, 10., 20.)], 101).unwrap();
            }
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "history::store::tests::crash_child",
                    "--ignored",
                    "--nocapture",
                ])
                .env("GTOP_HISTORY_CRASH_DIR", &f.0)
                .env("GTOP_HISTORY_CRASH_PHASE", phase)
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while !f.0.join("ready").exists() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let ready = f.0.join("ready").exists();
            child.kill().unwrap();
            child.wait().unwrap();
            assert!(ready, "crash child did not reach {phase}");
            let s = f.open(101);
            let buckets = s.buckets(100..102, None).unwrap();
            assert_eq!(buckets.len(), if phase == "after" { 2 } else { 1 });
            assert_eq!(buckets[0].processes["worker"].cpu_seconds, 20.);
            assert_eq!(
                s.connection
                    .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                    .unwrap(),
                "ok"
            );
        }
    }
}
