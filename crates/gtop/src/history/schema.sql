CREATE TABLE IF NOT EXISTS names(id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
CREATE TABLE IF NOT EXISTS system(
 resolution INTEGER NOT NULL, minute INTEGER NOT NULL,
 recorded INTEGER NOT NULL, seconds REAL NOT NULL,
 cpu REAL NOT NULL, memory REAL NOT NULL,
 gpu REAL, gpu_seconds REAL NOT NULL,
 temperature REAL, temp_seconds REAL NOT NULL,
 peak_cpu REAL NOT NULL, peak_memory REAL NOT NULL,
 peak_gpu REAL, peak_temp REAL,
 PRIMARY KEY(resolution,minute)
);
CREATE TABLE IF NOT EXISTS process(
 resolution INTEGER NOT NULL, minute INTEGER NOT NULL, name_id INTEGER NOT NULL,
 cpu_seconds REAL NOT NULL, gpu_seconds REAL NOT NULL,
 peak_memory INTEGER NOT NULL, peak_cpu REAL NOT NULL,
 PRIMARY KEY(resolution,minute,name_id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS process_name_minute ON process(name_id,resolution,minute);
CREATE TABLE IF NOT EXISTS meta(compacted_before INTEGER NOT NULL);
INSERT INTO meta SELECT 0 WHERE NOT EXISTS(SELECT 1 FROM meta);
CREATE TABLE IF NOT EXISTS migration(id INTEGER PRIMARY KEY CHECK(id=1), imported INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS rollup_meta(version INTEGER PRIMARY KEY CHECK(version=1));
-- Version 2 reserves -1 sensor seconds for the legacy missing-duration state.
-- Version 1 nonnegative durations retain their explicit values.
PRAGMA user_version=2;
