use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=GTOP_GIT_COMMIT");
    // The nearest `.git` entry selects which metadata the paths below resolve
    // to. Watching every candidate up to it reruns this script when that entry
    // is relinked, removed or added; a missing path makes Cargo always rerun.
    let manifest =
        std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    for dir in manifest.ancestors() {
        let entry = dir.join(".git");
        println!("cargo:rerun-if-changed={}", entry.display());
        if entry.exists() {
            break;
        }
    }
    for input in ["HEAD", "packed-refs"] {
        watch_git_path(input);
    }
    if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watch_git_path(&reference);
    }
    let commit = std::env::var("GTOP_GIT_COMMIT")
        .ok()
        .or_else(|| git(&["rev-parse", "--short", "HEAD"]));
    println!(
        "cargo:rustc-env=GTOP_GIT_COMMIT={}",
        commit
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "unknown".into())
    );
}

fn git(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn watch_git_path(name: &str) {
    if let Some(path) = git(&["rev-parse", "--git-path", name]) {
        println!("cargo:rerun-if-changed={path}");
    }
}
