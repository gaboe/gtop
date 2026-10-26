import importlib.util
from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path
import sqlite3
import sys
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("perf_report", Path(__file__).with_name("perf-report.py"))
report = importlib.util.module_from_spec(spec)
spec.loader.exec_module(report)


def insert_minutes(db, entries):
    sql = """INSERT INTO perf_minute (
        wall_seconds, process_cpu_seconds, visible_seconds, occluded_seconds,
        hidden_seconds, minimized_seconds, unknown_seconds, cpu_visible_seconds,
        cpu_occluded_seconds, cpu_hidden_seconds, cpu_minimized_seconds,
        cpu_unknown_seconds, draw_count, draw_total_ms, sampler_runs,
        sampler_total_ms, history_write_ms, refresh_interval_seconds,
        app_version, app_commit, mean_load_1m, mean_system_cpu_busy_percent,
        interval_start_seconds
    ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)"""
    for values, load, busy in entries:
        db.execute(sql, (*values[:20], load, busy, values[20]))


class PerfReportTests(unittest.TestCase):
    def test_report_splits_state_and_version_by_utc_hour(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "perf.sqlite"
            with sqlite3.connect(source) as db:
                schema = Path(__file__).parents[1] / "src" / "perf_schema.sql"
                db.executescript(schema.read_text())
                insert_minutes(db, [
                    ((3660, 0.8, 30, 0, 30, 0, 0, 0.6, 0, 0.2, 0, 0, 2, 1, 1, 2, 0.1, 3, "1.0", "a", 3600), 10., 30.),
                    ((3720, 1.6, 30, 0, 30, 0, 0, 1.2, 0, 0.4, 0, 0, 2, 1, 1, 2, 0.1, 3, "1.0", "a", 3600), 10., 50.),
                    ((7260, 0.5, 60, 0, 0, 0, 0, 0.5, 0, 0, 0, 0, 2, 1, 1, 2, 0.1, 3, "2.0", "b", 7200), 4., 20.),
                ])
            with tempfile.TemporaryDirectory() as scratch:
                copied = Path(scratch) / "copy.sqlite"
                report.copy_database(source, copied)
                summaries, work = report.summarize(copied)
            hidden = summaries[0]
            self.assertEqual((hidden.hour, hidden.version, hidden.state, hidden.load_bucket), ("1", "1.0", "hidden", "8-20"))
            self.assertEqual((hidden.minutes, hidden.median_core_pct, hidden.p90_core_pct, hidden.p95_core_pct), (2, 1.0, 1.3333333333333333, 1.3333333333333333))
            self.assertEqual((work[0].draws_per_min, work[0].mean_draw_ms, work[0].sampler_runs_per_min, work[0].mean_sampler_ms), (4 / 3, .5, 2 / 3, 2.))
            self.assertEqual(work[0].mean_system_cpu_busy_pct, 40.)
            visible = [row for row in summaries if row.version == "1.0" and row.state == "visible"]
            self.assertEqual(visible[0].median_core_pct, 3.0)
            self.assertEqual(visible[0].p90_core_pct, 4.0)
            self.assertEqual(visible[0].p95_core_pct, 4.0)
            self.assertEqual(len([row for row in summaries if row.hour == "2"]), 1)
            self.assertEqual(len([row for row in work if row.hour == "2"]), 1)

    def test_boundary_ending_and_crossing_minutes_use_start_hour(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "perf.sqlite"
            with sqlite3.connect(source) as db:
                schema = Path(__file__).parents[1] / "src" / "perf_schema.sql"
                db.executescript(schema.read_text())
                insert_minutes(db, [
                    ((end, 6, 60, 0, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, "1", "a", start), None, None)
                    for end, start in [(3600, 3540), (3630, 3570)]
                ])
            summary, work = report.summarize(source)
            self.assertEqual((summary[0].hour, summary[0].version, summary[0].state, summary[0].load_bucket), ("0", "1", "visible", "unknown"))
            self.assertEqual((summary[0].minutes, summary[0].median_core_pct, summary[0].p90_core_pct, summary[0].p95_core_pct), (2, 10., 10., 10.))
            self.assertEqual((work[0].draws_per_min, work[0].mean_draw_ms, work[0].sampler_runs_per_min, work[0].mean_sampler_ms), (0., None, 0., None))
            self.assertIsNone(work[0].mean_system_cpu_busy_pct)

    def test_report_splits_load_buckets_and_aggregates_draw_and_sampler_metrics(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "perf.sqlite"
            with sqlite3.connect(source) as db:
                schema = Path(__file__).parents[1] / "src" / "perf_schema.sql"
                db.executescript(schema.read_text())
                insert_minutes(db, [
                    ((3660, 1, 60, 0, 0, 0, 0, .6, 0, 0, 0, 0, 30, 90, 60, 120, 0, 3, "1", "a", 3600), 25., 60.),
                    ((3720, 1, 60, 0, 0, 0, 0, .3, 0, 0, 0, 0, 30, 150, 30, 90, 0, 3, "1", "a", 3660), 45., 70.),
                ])
            states, groups = report.summarize(source)
            self.assertEqual([row.load_bucket for row in groups], ["20-40", ">40"])
            self.assertEqual((groups[0].draws_per_min, groups[0].mean_draw_ms, groups[0].sampler_runs_per_min, groups[0].mean_sampler_ms), (30., 3., 60., 2.))
            self.assertEqual((groups[1].draws_per_min, groups[1].mean_draw_ms, groups[1].sampler_runs_per_min, groups[1].mean_sampler_ms), (30., 5., 30., 3.))
            self.assertEqual((groups[0].mean_system_cpu_busy_pct, groups[1].mean_system_cpu_busy_pct), (60., 70.))

    def test_rates_use_summed_interval_seconds(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "perf.sqlite"
            with sqlite3.connect(source) as db:
                schema = Path(__file__).parents[1] / "src" / "perf_schema.sql"
                db.executescript(schema.read_text())
                insert_minutes(db, [
                    ((3660, 1, 60, 0, 0, 0, 0, .6, 0, 0, 0, 0, 120, 600,
                      20, 400, 0, 3, "1.0", "a", 3600), 10., 50.),
                    ((3780, 1, 120, 0, 0, 0, 0, .6, 0, 0, 0, 0, 120, 600,
                      40, 800, 0, 3, "1.0", "a", 3660), 10., 50.),
                ])
            _, work = report.summarize(source)
            self.assertEqual(work[0].intervals, 2)
            self.assertEqual(work[0].draws_per_min, 80.)
            self.assertEqual(work[0].sampler_runs_per_min, 20.)

    def test_whole_interval_work_is_reported_once_without_state(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "perf.sqlite"
            with sqlite3.connect(source) as db:
                schema = Path(__file__).parents[1] / "src" / "perf_schema.sql"
                db.executescript(schema.read_text())
                insert_minutes(db, [
                    ((3660, 1, 30, 0, 30, 0, 0, .6, 0, .4, 0, 0, 120, 600,
                      20, 400, 0, 3, "1.0", "a", 3600), 10., 50.),
                ])
            output = StringIO()
            with patch.object(sys, "argv", ["perf-report.py", str(source)]), redirect_stdout(output):
                self.assertEqual(report.main(), 0)
            lines = output.getvalue().splitlines()
            self.assertEqual(lines[1:3], [
                "1\t1.0\thidden\t8-20\t1\t1.333\t1.333\t1.333",
                "1\t1.0\tvisible\t8-20\t1\t2.000\t2.000\t2.000",
            ])
            self.assertEqual(lines[4], "hour_utc\tversion\tload_bucket\tintervals\tdraws_per_min\tmean_draw_ms\tsampler_runs_per_min\tmean_sampler_ms\tmean_system_cpu_busy_pct")
            self.assertEqual(lines[5], "1\t1.0\t8-20\t1\t120.000\t5.000\t20.000\t20.000\t50.000")

    def test_load_bucket_edges(self):
        self.assertEqual([report.load_bucket(value) for value in (None, 7.9, 8., 20., 40.)], [
            "unknown", "<8", "8-20", "20-40", ">40"
        ])

    def test_main_prints_load_and_performance_columns(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "perf.sqlite"
            with sqlite3.connect(source) as db:
                schema = Path(__file__).parents[1] / "src" / "perf_schema.sql"
                db.executescript(schema.read_text())
                insert_minutes(db, [
                    ((3660, 1, 60, 0, 0, 0, 0, .6, 0, 0, 0, 0, 30, 90, 60, 120, 0, 3, "1", "a", 3600), 25., 60.),
                ])
            output = StringIO()
            with patch.object(sys, "argv", ["perf-report.py", str(source)]), redirect_stdout(output):
                self.assertEqual(report.main(), 0)
            self.assertEqual(output.getvalue().splitlines(), [
                "hour_utc\tversion\tstate\tload_bucket\tminutes\tmedian_core_pct\tp90_core_pct\tp95_core_pct",
                "1\t1\tvisible\t20-40\t1\t1.000\t1.000\t1.000",
                "",
                "hour_utc\tversion\tload_bucket\tintervals\tdraws_per_min\tmean_draw_ms\tsampler_runs_per_min\tmean_sampler_ms\tmean_system_cpu_busy_pct",
                "1\t1\t20-40\t1\t30.000\t3.000\t60.000\t2.000\t60.000",
            ])

    def test_copy_uses_read_only_source_connection(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "perf.sqlite"
            destination = Path(directory) / "copy.sqlite"
            sqlite3.connect(source).close()
            connect = sqlite3.connect
            opened = []

            def track(database, *args, **kwargs):
                opened.append((database, kwargs))
                return connect(database, *args, **kwargs)

            with patch.object(report.sqlite3, "connect", side_effect=track):
                report.copy_database(source, destination)
            self.assertIn("mode=ro", opened[0][0])
            self.assertEqual(opened[0][1]["uri"], True)


if __name__ == "__main__":
    unittest.main()
