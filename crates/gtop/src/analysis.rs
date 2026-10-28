use serde::Deserialize;
use std::{collections::HashMap, sync::OnceLock};

const HISTORY_MINUTES: u64 = 24 * 60;
const HISTORY_COLUMNS: usize = 144;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub what: String,
    pub kind: String,
    pub about: String,
    pub source: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessHistory {
    pub columns: Vec<Option<f32>>,
    pub pattern: String,
    pub core_minutes: f64,
    pub busy_average: f64,
    pub peak_cpu: f64,
    pub peak_minute: u64,
    pub longest_busy_minutes: usize,
    pub peak_memory: u64,
    pub recorded_minutes: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessAnalysis {
    pub identity: ProcessIdentity,
    pub history: Option<ProcessHistory>,
}

pub fn analyze<'a>(
    name: &str,
    command: &str,
    now_minute: u64,
    buckets: impl IntoIterator<Item = &'a crate::history::Bucket>,
) -> ProcessAnalysis {
    ProcessAnalysis {
        identity: identity(name, command),
        history: process_history(name, now_minute, buckets),
    }
}

fn process_history<'a>(
    name: &str,
    now_minute: u64,
    buckets: impl IntoIterator<Item = &'a crate::history::Bucket>,
) -> Option<ProcessHistory> {
    use std::collections::BTreeMap;

    let start_minute = now_minute.saturating_sub(HISTORY_MINUTES - 1);
    let buckets: BTreeMap<_, _> = buckets
        .into_iter()
        .filter(|bucket| bucket.minute >= start_minute && bucket.minute <= now_minute)
        .map(|bucket| (bucket.minute, bucket))
        .collect();
    let seen = buckets
        .values()
        .any(|bucket| bucket.processes.contains_key(name));
    if !seen {
        return None;
    }

    let mut series = Vec::with_capacity(HISTORY_MINUTES as usize);
    let mut core_seconds = 0.;
    let mut peak_cpu = 0.;
    let mut peak_minute = start_minute;
    let mut peak_memory = 0;
    for minute in start_minute..=now_minute {
        let Some(bucket) = buckets.get(&minute) else {
            series.push(None);
            continue;
        };
        let total = bucket.processes.get(name);
        let cpu = total
            .filter(|_| bucket.seconds > 0.)
            .map(|total| total.cpu_seconds / bucket.seconds * 100.)
            .unwrap_or(0.);
        core_seconds += total.map(|total| total.cpu_seconds).unwrap_or(0.);
        if let Some(total) = total {
            if total.peak_cpu as f64 > peak_cpu {
                peak_cpu = total.peak_cpu as f64;
                peak_minute = minute;
            }
            peak_memory = peak_memory.max(total.peak_memory);
        }
        series.push(Some((cpu, total.is_some())));
    }

    let recorded: Vec<_> = series.iter().flatten().collect();
    let active: Vec<_> = recorded
        .iter()
        .filter_map(|sample| {
            let (cpu, has_process) = **sample;
            (has_process && cpu > 5.).then_some(cpu)
        })
        .collect();
    let busy_average = if active.is_empty() {
        0.
    } else {
        active.iter().sum::<f64>() / active.len() as f64
    };
    let share = active.len() as f64 / recorded.len().max(1) as f64;
    let pattern = if share > 0.8 {
        format!(
            "Runs steadily: busy in {} % of recorded minutes.",
            (share * 100.).round() as usize
        )
    } else if share > 0.25 {
        format!(
            "Busy often: {} % of recorded minutes.",
            (share * 100.).round() as usize
        )
    } else {
        format!(
            "Runs in bursts: busy in {} of {} recorded minutes.",
            active.len(),
            recorded.len()
        )
    };
    let mut longest_busy_minutes = 0;
    let mut busy_run = 0;
    for sample in &series {
        match sample {
            Some((cpu, _)) if *cpu > 50. => {
                busy_run += 1;
                longest_busy_minutes = longest_busy_minutes.max(busy_run);
            }
            _ => busy_run = 0,
        }
    }

    let columns = series
        .chunks(10)
        .take(HISTORY_COLUMNS)
        .map(|minutes| {
            let recorded: Vec<_> = minutes.iter().flatten().map(|(cpu, _)| *cpu).collect();
            (!recorded.is_empty())
                .then(|| (recorded.iter().sum::<f64>() / recorded.len() as f64) as f32)
        })
        .collect();

    Some(ProcessHistory {
        columns,
        pattern,
        core_minutes: core_seconds / 60.,
        busy_average,
        peak_cpu,
        peak_minute,
        longest_busy_minutes,
        peak_memory,
        recorded_minutes: recorded.len(),
    })
}

pub fn current_cpu(processes: &[gtop_core::ProcessInfo], name: &str) -> f32 {
    processes
        .iter()
        .filter(|process| process.name == name)
        .map(|process| process.cpu_usage)
        .sum()
}

pub fn now_sentence(
    current_cpu: f32,
    busy_average: f64,
    logical_cpus: usize,
    scale: crate::palette::CpuScale,
) -> String {
    sample_sentence("Now", current_cpu, busy_average, logical_cpus, scale)
}

pub fn sample_sentence(
    when: &str,
    current_cpu: f32,
    busy_average: f64,
    logical_cpus: usize,
    scale: crate::palette::CpuScale,
) -> String {
    let ratio = if busy_average > 0. {
        current_cpu as f64 / busy_average
    } else {
        0.
    };
    let current_share = crate::model::format_cpu_share(current_cpu as f64, logical_cpus, scale);
    if ratio > 1.5 {
        format!("{when} {current_share} %, {ratio:.1}× its usual busy load.")
    } else if busy_average > 0. && ratio < 0.5 {
        format!("{when} {current_share} %, well below its usual busy load.")
    } else {
        format!("{when} {current_share} %, about its usual busy load.")
    }
}

#[derive(Deserialize)]
struct CatalogEntry {
    name: String,
    what: String,
    kind: String,
    about: String,
}

fn catalog() -> &'static HashMap<String, CatalogEntry> {
    static CATALOG: OnceLock<HashMap<String, CatalogEntry>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str::<Vec<CatalogEntry>>(include_str!("../assets/catalog.json"))
            .expect("embedded process catalog is valid")
            .into_iter()
            .map(|entry| (entry.name.clone(), entry))
            .collect()
    })
}

pub fn identity(name: &str, command: &str) -> ProcessIdentity {
    if let Some(entry) = catalog().get(name) {
        return ProcessIdentity {
            what: entry.what.clone(),
            kind: entry.kind.clone(),
            about: entry.about.clone(),
            source: "gtop catalog",
        };
    }
    if let Some((binary, hash)) = name.rsplit_once('-') {
        if !binary.is_empty()
            && hash.len() == 16
            && hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return ProcessIdentity {
                what: format!("Rust test or build binary of {binary}"),
                kind: "Developer tool".into(),
                about: "Cargo adds a hash to test and build binary names.".into(),
                source: "from its name",
            };
        }
    }
    if name
        .strip_suffix(".Tests")
        .is_some_and(|project| !project.is_empty())
    {
        return ProcessIdentity {
            what: format!(".NET test project {name}"),
            kind: "Developer tool".into(),
            about: "Runs tests for this .NET project.".into(),
            source: "from its name",
        };
    }
    if command.starts_with("/System/") || command.starts_with("/usr/libexec/") {
        return ProcessIdentity {
            what: "macOS system process".into(),
            kind: "macOS".into(),
            about: "Ships with macOS.".into(),
            source: "from its path",
        };
    }
    if let Some(app) = command
        .split('/')
        .find(|part| part.ends_with(".app"))
        .map(|part| part.trim_end_matches(".app"))
    {
        return ProcessIdentity {
            what: format!("Part of {app}"),
            kind: "App helper".into(),
            about: format!("Runs from inside {app}.app."),
            source: "from its path",
        };
    }
    ProcessIdentity {
        what: "Unknown process".into(),
        kind: "Unknown".into(),
        about: "Not in gtop's catalog and its path says nothing more.".into(),
        source: "no match",
    }
}

#[cfg(test)]
mod tests {
    use super::{identity, process_history};
    use crate::history::{Bucket, ProcessTotal};
    use std::collections::BTreeMap;

    fn bucket(minute: u64, cpu: f32) -> Bucket {
        let mut processes = BTreeMap::new();
        processes.insert(
            "builder".into(),
            ProcessTotal {
                cpu_seconds: cpu as f64 / 100. * 60.,
                peak_memory: 1024,
                peak_cpu: cpu,
                ..Default::default()
            },
        );
        Bucket {
            minute,
            seconds: 60.,
            processes,
            ..Default::default()
        }
    }

    #[test]
    fn build_and_test_names_resolve_before_path_fallback() {
        for name in [
            "mini_speed_reader_firmware-d89a92ccf166f61b",
            "mini_speed_reader_firmware-796e1d60ea54cad7",
            "mini_speed_reader_firmware-5d351cac350b054f",
            "mini_speed_reader_firmware-933c8b697cfdb27a",
            "mini_speed_reader_firmware-54941cf2cd547ec6",
            "mini_speed_reader_firmware-aff03edcda2a13be",
            "account_tests-f7d829b1817a0b4a",
            "readmaxer-d9b077a1e2074261",
            "library_routes_tests-f724a15104d2ece7",
            "acta-ef192c1ed6ad40e5",
            "acta-568ba7485530c996",
        ] {
            let found = identity(name, "/System/fixture");
            assert_eq!(
                found.what,
                format!(
                    "Rust test or build binary of {}",
                    name.rsplit_once('-').unwrap().0
                )
            );
            assert_eq!(found.kind, "Developer tool");
            assert_eq!(found.source, "from its name");
        }
        for name in [
            "SabFx.Cqrs.Wolverine.Tests",
            "AppFx.Workflows.Tests",
            "Nexus.Roslyn.DomainLayer.Analyzers.Tests",
            "Nexus.Roslyn.InfrastructureLayer.Analyzers.Tests",
            "Nexus.Roslyn.Common.CodeFixes.Tests",
        ] {
            let found = identity(name, "/Applications/Runner.app/runner");
            assert_eq!(found.what, format!(".NET test project {name}"));
            assert_eq!(found.kind, "Developer tool");
        }
        for name in [
            "-d89a92ccf166f61b",
            "app-D89a92ccf166f61b",
            "app-d89a92ccf166f61",
            "app-d89a92ccf166f61bb",
            "app-d89a92ccf166f61g",
            ".Tests",
            "app.tests",
            "app.Tests.dll",
            "acta-final",
            "base-barrier",
        ] {
            assert_eq!(
                identity(name, "/tmp/fixture").what,
                "Unknown process",
                "{name}"
            );
        }
        assert_eq!(identity("rustc", "/System/fixture").source, "gtop catalog");
    }

    #[test]
    fn catalog_identifies_dotnet_compiler_and_development_tools() {
        let compiler = super::identity("csc", "/sdk/Roslyn/csc");
        assert_eq!(compiler.what, "C# compiler");
        assert_eq!(compiler.source, "gtop catalog");
        for name in [
            "VBCSCompiler",
            "dotnet",
            "MSBuild",
            "msbuild",
            "testhost",
            "vstest.console",
            "Rider",
            "ReSharperHost",
            "OrbStack",
            "docker",
            "csharp-ls",
            "deno",
            "gradle",
            "Python",
            "esbuild",
            "zed",
        ] {
            assert_eq!(super::identity(name, name).source, "gtop catalog", "{name}");
        }
    }

    #[test]
    fn now_sentence_uses_cpu_scale_and_includes_zero() {
        let snapshot = crate::test_data::snapshot();
        assert_eq!(super::current_cpu(&snapshot.processes, "rustc"), 350.);
        assert_eq!(
            super::now_sentence(350., 350., 14, crate::palette::CpuScale::Mac),
            "Now 25.0 %, about its usual busy load."
        );
        assert_eq!(
            super::now_sentence(350., 350., 14, crate::palette::CpuScale::Core),
            "Now 350.0 %, about its usual busy load."
        );
        assert!(
            super::now_sentence(0., 350., 14, crate::palette::CpuScale::Mac,)
                .contains("below its usual")
        );
    }

    #[test]
    fn executable_identity_does_not_use_an_app_argument() {
        let mut process = crate::test_data::process(1, "custom-runner", 0.);
        process.executable = "/opt/bin/custom-runner".into();
        process.argv = vec![
            process.executable.clone(),
            "/Applications/Slack.app/Contents/script.py".into(),
        ];
        assert_eq!(
            identity(&process.name, &process.executable).source,
            "no match"
        );
        assert_eq!(
            identity("custom", "/Applications/My App.app/Contents/MacOS/helper").what,
            "Part of My App"
        );
    }

    #[test]
    fn finds_catalog_entries_and_names_the_source() {
        let identity = identity("rustc", "/opt/homebrew/bin/rustc");
        assert_eq!(identity.what, "Rust compiler");
        assert_eq!(identity.kind, "Developer tool");
        assert_eq!(identity.source, "gtop catalog");
    }

    #[test]
    fn identifies_app_helpers_from_their_path() {
        let identity = identity("helper", "/Applications/Slack.app/Contents/MacOS/helper");
        assert_eq!(identity.what, "Part of Slack");
        assert_eq!(identity.kind, "App helper");
        assert_eq!(identity.source, "from its path");
    }

    #[test]
    fn identifies_system_processes_from_system_paths() {
        for command in [
            "/System/Library/CoreServices/Finder.app/finder",
            "/usr/libexec/xpcproxy",
        ] {
            let identity = identity("other", command);
            assert_eq!(identity.what, "macOS system process");
            assert_eq!(identity.source, "from its path");
        }
    }

    #[test]
    fn names_unknown_processes_and_the_missing_match_source() {
        let identity = identity("unlisted", "some-command");
        assert_eq!(identity.what, "Unknown process");
        assert_eq!(identity.kind, "Unknown");
        assert_eq!(identity.source, "no match");
    }

    #[test]
    fn history_is_absent_when_the_name_was_never_recorded() {
        assert!(process_history("builder", 1439, std::iter::empty()).is_none());
    }

    #[test]
    fn pattern_thresholds_match_the_behavior_rules() {
        let often = (0..5)
            .map(|minute| bucket(minute, if minute < 4 { 20. } else { 0. }))
            .collect::<Vec<_>>();
        assert_eq!(
            process_history("builder", 1439, &often).unwrap().pattern,
            "Busy often: 80 % of recorded minutes."
        );
        let bursts = (0..4)
            .map(|minute| bucket(minute, if minute == 0 { 20. } else { 0. }))
            .collect::<Vec<_>>();
        assert_eq!(
            process_history("builder", 1439, &bursts).unwrap().pattern,
            "Runs in bursts: busy in 1 of 4 recorded minutes."
        );
        let steady = (0..6)
            .map(|minute| bucket(minute, if minute < 5 { 20. } else { 0. }))
            .collect::<Vec<_>>();
        assert_eq!(
            process_history("builder", 1439, &steady).unwrap().pattern,
            "Runs steadily: busy in 83 % of recorded minutes."
        );
    }

    #[test]
    fn now_sentence_uses_strict_ratio_thresholds() {
        assert_eq!(
            super::now_sentence(60., 40., 1, crate::palette::CpuScale::Core),
            "Now 60.0 %, about its usual busy load."
        );
        assert_eq!(
            super::now_sentence(80., 40., 1, crate::palette::CpuScale::Core),
            "Now 80.0 %, 2.0× its usual busy load."
        );
        assert_eq!(
            super::now_sentence(10., 40., 1, crate::palette::CpuScale::Core),
            "Now 10.0 %, well below its usual busy load."
        );
        assert_eq!(
            super::now_sentence(20., 40., 1, crate::palette::CpuScale::Core),
            "Now 20.0 %, about its usual busy load."
        );
    }

    #[test]
    fn longest_busy_run_resets_at_gaps_and_tape_keeps_gap_columns() {
        let buckets = [
            bucket(0, 60.),
            bucket(1, 60.),
            bucket(2, 60.),
            bucket(4, 60.),
            bucket(5, 60.),
            bucket(20, 5.),
        ];
        let history = process_history("builder", 1439, &buckets).unwrap();
        assert_eq!(history.longest_busy_minutes, 3);
        assert_eq!(history.columns.len(), 144);
        assert!(history.columns[0].is_some());
        assert!(history.columns[1].is_none());
        assert!(history.columns[2].is_some());
        assert_eq!(history.recorded_minutes, 6);
        assert_eq!(history.peak_memory, 1024);
        assert_eq!(history.core_minutes, 3.05);
        assert_eq!(history.busy_average, 60.);
        assert_eq!(history.peak_cpu, 60.);
        assert_eq!(history.peak_minute, 0);
    }
}
