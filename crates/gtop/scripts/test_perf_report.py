import importlib.util
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("perf_report", Path(__file__).with_name("perf-report.py"))
report = importlib.util.module_from_spec(spec)
spec.loader.exec_module(report)


class PerfReportTests(unittest.TestCase):
    def test_report_splits_state_and_version_by_utc_hour(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "perf.sqlite"
            with sqlite3.connect(source) as db:
                schema = Path(__file__).parents[1] / "src" / "perf_schema.sql"
                db.executescript(schema.read_text())
                db.executemany(
                    "INSERT INTO perf_minute VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                    [
                        (3600, 0.8, 30, 0, 30, 0, 0, 0.6, 0, 0.2, 0, 0, 2, 1, 1, 2, 0.1, 3, "1.0", "a"),
                        (3660, 1.6, 30, 0, 30, 0, 0, 1.2, 0, 0.4, 0, 0, 2, 1, 1, 2, 0.1, 3, "1.0", "a"),
                        (7200, 0.5, 60, 0, 0, 0, 0, 0.5, 0, 0, 0, 0, 2, 1, 1, 2, 0.1, 3, "2.0", "b"),
                    ],
                )
            with tempfile.TemporaryDirectory() as scratch:
                copied = Path(scratch) / "copy.sqlite"
                report.copy_database(source, copied)
                summaries = report.summarize(copied)
            self.assertEqual(summaries[0], ("1", "1.0", "hidden", 2, 1.0, 1.3333333333333333, 1.3333333333333333))
            visible = [row for row in summaries if row[1:] and row[1] == "1.0" and row[2] == "visible"]
            self.assertEqual(visible[0][4], 3.0)
            self.assertEqual(visible[0][5], 4.0)
            self.assertEqual(visible[0][6], 4.0)
            self.assertEqual(len([row for row in summaries if row[0] == "2"]), 1)

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
