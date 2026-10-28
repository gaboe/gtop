import importlib.util
import io
import json
import multiprocessing
import os
from pathlib import Path
import sys
import tempfile
import time
import unittest
from contextlib import redirect_stdout
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("draw_benchmark", Path(__file__).with_name("draw-benchmark.py"))
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)
LOCK = b'[[package]]\nname = "gpui-pre"\nversion = "0.3.9"\nsource = "registry+example"\nchecksum = "abc"\ndependencies = []\n'


def overlap(root):
    bench.__file__ = str(root / "crates/gtop/scripts/draw-benchmark.py")
    def preparation(source, scratch):
        with (root / "events").open("a") as log:
            log.write(f"start {time.monotonic()}\n")
            log.flush()
            time.sleep(0.2)
            log.write(f"end {time.monotonic()}\n")
        raise SystemExit(0)
    with patch.object(bench, "prepare", preparation), patch.object(sys, "argv", ["benchmark"]), patch.dict(os.environ, {"CARGO_HOME": str(root / "cargo")}):
        bench.main()


class StateTests(unittest.TestCase):
    def test_failed_build_prints_diagnostic_and_restores_lock(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            lock = root / "Cargo.lock"
            lock.write_bytes(LOCK)
            def failed(*args, **kwargs):
                lock.write_bytes(bench.patched_lock(LOCK))
                diagnostic = {"reason": "compiler-message", "message": {"rendered": "error: benchmark fixture failed"}}
                return bench.subprocess.CompletedProcess([], 101, stdout=json.dumps(diagnostic))
            output = io.StringIO()
            with patch.object(bench.subprocess, "run", failed), redirect_stdout(output), self.assertRaises(bench.subprocess.CalledProcessError):
                bench.run_child([], root, {}, capture=True)
            self.assertIn("error: benchmark fixture failed", output.getvalue())
            self.assertEqual(lock.read_bytes(), LOCK)

    def test_child_success_failure_and_conflict(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            lock = root / "Cargo.lock"
            for code in (0, 7):
                lock.write_bytes(LOCK)
                def child(*args, **kwargs):
                    lock.write_bytes(bench.patched_lock(LOCK))
                    return code
                with patch.object(bench.subprocess, "call", child):
                    self.assertEqual(bench.run_child([], root, {}), code)
                self.assertEqual(lock.read_bytes(), LOCK)
            lock.write_bytes(LOCK)
            def failed(*args, **kwargs):
                lock.write_bytes(bench.patched_lock(LOCK))
                raise OSError("child failed")
            with patch.object(bench.subprocess, "call", failed), self.assertRaises(OSError):
                bench.run_child([], root, {})
            self.assertEqual(lock.read_bytes(), LOCK)
            lock.write_bytes(LOCK)
            def captured(*args, **kwargs):
                lock.write_bytes(bench.patched_lock(LOCK))
                return bench.subprocess.CompletedProcess([], 0, stdout="{}")
            with patch.object(bench.subprocess, "run", captured):
                self.assertEqual(bench.run_child([], root, {}, capture=True).returncode, 0)
            self.assertEqual(lock.read_bytes(), LOCK)
            def conflict(*args, **kwargs):
                lock.write_bytes(b"concurrent valid update")
                return 0
            with patch.object(bench.subprocess, "call", conflict), self.assertRaisesRegex(RuntimeError, "preserved"):
                bench.run_child([], root, {})
            self.assertEqual(lock.read_bytes(), b"concurrent valid update")

    def test_overlapping_invocations_serialize_before_preparation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "cargo/registry/src/test/gpui-pre-0.3.9").mkdir(parents=True)
            context = multiprocessing.get_context("fork")
            children = [context.Process(target=overlap, args=(root,)) for _ in range(2)]
            for child in children:
                child.start()
            for child in children:
                child.join(5)
                self.assertEqual(child.exitcode, 0)
            events = (root / "events").read_text().splitlines()
            self.assertEqual([event.split()[0] for event in events], ["start", "end", "start", "end"])

    def test_invalid_dimensions_rejected_before_cargo(self):
        for name in ("PROCESSES", "CORES", "WIDTH", "HEIGHT"):
            for value in ("0", "abc", "4294967297"):
                env = dict(os.environ, **{"GTOP_DRAW_" + name: value})
                result = bench.subprocess.run([sys.executable, str(Path(__file__).with_name("draw-benchmark.py"))], env=env, capture_output=True)
                self.assertEqual(result.returncode, 2)
                self.assertIn(("GTOP_DRAW_" + name).encode(), result.stderr)

    def test_prepared_cache_integrity_and_upstream_identity(self):
        source = list((Path.home() / ".cargo/registry/src").glob("*/gpui-pre-0.3.9"))[0]
        with tempfile.TemporaryDirectory() as directory:
            scratch = Path(directory) / "copy"
            identity = bench.prepare(source, scratch)
            before = (scratch / "src/window.rs").stat().st_mtime_ns
            self.assertEqual(bench.prepare(source, scratch), identity)
            self.assertEqual((scratch / "src/window.rs").stat().st_mtime_ns, before)
            (scratch / "src/app.rs").write_text("corrupt")
            self.assertEqual(bench.prepare(source, scratch), identity)
            self.assertNotEqual((scratch / "src/app.rs").read_text(), "corrupt")
            upstream = Path(directory) / "upstream"
            bench.shutil.copytree(source, upstream)
            for name in ("window.rs", "element.rs", "app.rs", "app/test_context.rs", "platform.rs", "gpui.rs"):
                path = upstream / "src" / name
                original = path.read_bytes()
                path.write_bytes(original + b"\n")
                self.assertNotEqual(bench.prepare(upstream, scratch), identity)
                path.write_bytes(original)
            marker = json.loads((scratch / ".draw-benchmark-source").read_text())
            self.assertEqual(marker["output"], bench.tree_digest(scratch))


if __name__ == "__main__":
    unittest.main()
