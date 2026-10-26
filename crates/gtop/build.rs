use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=GTOP_GIT_COMMIT");
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
