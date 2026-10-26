use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=GTOP_GIT_COMMIT");
    let commit = std::env::var("GTOP_GIT_COMMIT").ok().or_else(|| {
        Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    });
    println!(
        "cargo:rustc-env=GTOP_GIT_COMMIT={}",
        commit
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "unknown".into())
    );
}
