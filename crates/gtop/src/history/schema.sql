CREATE TABLE IF NOT EXISTS names(id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
CREATE TABLE IF NOT EXISTS system(
 resolution INTEGER NOT NULL, minute INTEGER NOT NULL,
 recorded INTEGER NOT NULL, seconds REAL NOT NULL,
 cpu REAL NOT NULL, memory REAL NOT NULL,
 gpu REAL, gpu_seconds REAL NOT NULL,
 temperature REAL, temp_seconds REAL NOT NULL,
 peak_cpu REAL NOT NULL, peak_memory REAL NOT NULL,
 peak_gpu REAL, peak_temp REAL,
 network_rx_bytes NUMERIC,
 network_rx_peak REAL,
 network_rx_seconds REAL,
 network_tx_bytes NUMERIC,
 network_tx_peak REAL,
 network_tx_seconds REAL,
 disk_read_bytes NUMERIC,
 disk_read_peak REAL,
 disk_read_seconds REAL,
 disk_write_bytes NUMERIC,
 disk_write_peak REAL,
 disk_write_seconds REAL,
 disk_free REAL,
 disk_free_seconds REAL,
 PRIMARY KEY(resolution,minute)
);
-- NUMERIC packs integral floats. Work aggregates use TOTAL to avoid integer overflow.
CREATE TABLE IF NOT EXISTS process(
 resolution INTEGER NOT NULL, minute INTEGER NOT NULL, name_id INTEGER NOT NULL,
 cpu_seconds NUMERIC NOT NULL, gpu_seconds NUMERIC NOT NULL,
 peak_memory INTEGER NOT NULL, peak_cpu NUMERIC NOT NULL,
 disk_read_bytes NUMERIC NOT NULL DEFAULT 0, disk_write_bytes NUMERIC NOT NULL DEFAULT 0,
 PRIMARY KEY(resolution,minute,name_id)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS presence(
 resolution INTEGER NOT NULL, minute INTEGER NOT NULL, ids BLOB NOT NULL,
 PRIMARY KEY(resolution,minute)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS meta(compacted_before INTEGER NOT NULL);
INSERT INTO meta SELECT 0 WHERE NOT EXISTS(SELECT 1 FROM meta);
CREATE TABLE IF NOT EXISTS migration(id INTEGER PRIMARY KEY CHECK(id=1), imported INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS rollup_meta(version INTEGER PRIMARY KEY CHECK(version=1));
PRAGMA user_version=4;
