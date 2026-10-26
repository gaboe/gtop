import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class BuildMetadataTests(unittest.TestCase):
    def test_incremental_build_tracks_external_git_metadata(self):
        root = Path(__file__).resolve().parents[3]
        target = root / "target" / "metadata-tests"
        target.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=target) as directory:
            scratch = Path(directory)
            repo = scratch / "package"
            repo.mkdir()
            env = dict(os.environ, CARGO_TARGET_DIR=str(scratch / "target"),
                       GIT_CEILING_DIRECTORIES=str(scratch))
            env.pop("GTOP_GIT_COMMIT", None)
            env.update(GIT_AUTHOR_NAME="Test", GIT_AUTHOR_EMAIL="test@example.com",
                       GIT_COMMITTER_NAME="Test", GIT_COMMITTER_EMAIL="test@example.com")

            def run(*args):
                return subprocess.check_output(args, cwd=repo, env=env, text=True).strip()

            run("git", "init", "--quiet", "--separate-git-dir", str(scratch / "git"))
            (repo / "Cargo.toml").write_text('[package]\nname="metadata-probe"\nversion="0.1.0"\nedition="2021"\n[workspace]\n')
            (repo / "src").mkdir()
            (repo / "src/main.rs").write_text('fn main() { println!("{}", env!("GTOP_GIT_COMMIT")); }\n')
            shutil.copyfile(root / "crates/gtop/build.rs", repo / "build.rs")
            run("git", "add", ".")
            run("git", "commit", "--quiet", "-m", "initial")
            self.assertEqual(run("cargo", "run", "--offline", "--quiet"), run("git", "rev-parse", "--short", "HEAD"))
            run("git", "commit", "--quiet", "--allow-empty", "-m", "next")
            self.assertEqual(run("cargo", "run", "--offline", "--quiet"), run("git", "rev-parse", "--short", "HEAD"))
            run("git", "pack-refs", "--all", "--prune")
            self.assertEqual(run("cargo", "run", "--offline", "--quiet"), run("git", "rev-parse", "--short", "HEAD"))
            run("git", "checkout", "--quiet", "--detach", "HEAD~1")
            self.assertEqual(run("cargo", "run", "--offline", "--quiet"), run("git", "rev-parse", "--short", "HEAD"))
            other = scratch / "other-git"
            run("git", "init", "--quiet", "--bare", str(other))
            tree = subprocess.check_output(["git", "--git-dir", str(other), "mktree"], input="", env=env, text=True).strip()
            commit = subprocess.check_output(["git", "--git-dir", str(other), "commit-tree", tree, "-m", "other"], env=env, text=True).strip()
            subprocess.check_call(["git", "--git-dir", str(other), "update-ref", "refs/heads/main", commit], env=env)
            subprocess.check_call(["git", "--git-dir", str(other), "symbolic-ref", "HEAD", "refs/heads/main"], env=env)
            (repo / ".git").write_text(f"gitdir: {other}\n")
            self.assertEqual(run("cargo", "run", "--offline", "--quiet"), commit[:len(run("git", "rev-parse", "--short", "HEAD"))])
            (repo / ".git").unlink()
            self.assertEqual(run("cargo", "run", "--offline", "--quiet"), "unknown")
            (repo / ".git").write_text(f"gitdir: {scratch / 'git'}\n")
            env["GTOP_GIT_COMMIT"] = "explicit-reproducible"
            self.assertEqual(run("cargo", "run", "--offline", "--quiet"), "explicit-reproducible")
            env.pop("GTOP_GIT_COMMIT")
            (repo / ".git").unlink()
            self.assertEqual(run("cargo", "run", "--offline", "--quiet"), "unknown")


if __name__ == "__main__":
    unittest.main()
