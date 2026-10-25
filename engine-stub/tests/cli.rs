use assert_cmd::Command;
use predicates::str::contains;
use serde_json::Value;
use std::fs;
use std::path::Path;

fn stub() -> Command {
    let mut c = Command::cargo_bin("engine-stub").unwrap();
    c.env_remove("BACKSAVE_STUB_MODE");
    c
}

fn log_lines(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn ok_copies_file_and_writes_log() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("model_2026.rvt");
    let out = dir.path().join("out.rvt");
    let log = dir.path().join("log.jsonl");
    fs::write(&input, b"some bytes \x00\x01\x02").unwrap();
    stub()
        .args(["downgrade"])
        .arg(&input)
        .args(["--to", "2024", "--out"])
        .arg(&out)
        .arg("--log")
        .arg(&log)
        .assert()
        .success();
    assert_eq!(fs::read(&input).unwrap(), fs::read(&out).unwrap());
    let lines = log_lines(&log);
    assert_eq!(lines[0]["type"], "run");
    let dropped: Vec<_> = lines
        .iter()
        .filter(|l| l["type"] == "element_dropped")
        .collect();
    assert_eq!(dropped.len(), 72);
    let count = |class: &str, reason: &str| {
        dropped
            .iter()
            .filter(|l| l["class"] == class && l["reason"] == reason)
            .count()
    };
    assert_eq!(count("CustomElement", "no_rule"), 61);
    assert_eq!(count("MEPSystemZoneElementType", "no_rule"), 2);
    assert_eq!(count("ElementHeader", "cascade"), 9);
    assert!(
        dropped
            .iter()
            .all(|l| l["id"].is_u64() && l.get("rule").is_some() && l.get("method_doc").is_some())
    );
    assert!(
        dropped
            .iter()
            .filter(|l| l["reason"] == "cascade")
            .all(|l| l["cascade_from"].is_u64())
    );
    assert!(
        dropped
            .iter()
            .filter(|l| l["reason"] != "cascade")
            .all(|l| l["cascade_from"].is_null())
    );
    assert_eq!(
        lines
            .iter()
            .filter(|l| l["type"] == "field_dropped")
            .count(),
        1
    );
    assert_eq!(lines.last().unwrap()["type"], "summary");
}

fn run_mode(
    name: &str,
) -> (
    assert_cmd::assert::Assert,
    std::path::PathBuf,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join(name);
    let out = dir.path().join("out.rvt");
    let log = dir.path().join("log.jsonl");
    fs::write(&input, b"x").unwrap();
    let a = stub()
        .arg("downgrade")
        .arg(&input)
        .args(["--to", "2024", "--out"])
        .arg(&out)
        .arg("--log")
        .arg(&log)
        .assert();
    (a, out, dir)
}

#[test]
fn refuse_exits_3_with_message() {
    let (a, _, _d) = run_mode("refuse_2026.rvt");
    a.code(3)
        .stderr(contains("Worksharing central files are not supported yet"));
}

#[test]
fn corrupt_exits_4() {
    let (a, _, _d) = run_mode("corrupt_2026.rvt");
    a.code(4);
}

#[test]
fn fail_exits_5_without_output() {
    let (a, out, _d) = run_mode("fail_2026.rvt");
    a.code(5);
    assert!(!out.exists());
}

#[test]
fn probe_reads_year_and_kind_from_name() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("a_2025.rfa");
    fs::write(&f, b"x").unwrap();
    let out = stub().arg("probe").arg(&f).arg("--json").assert().success();
    let v: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(
        v,
        serde_json::json!({"kind":"rfa","year":2025,"targets":[2024,2023]})
    );
}

#[test]
fn capabilities_lists_pairs_2023_2027() {
    let out = stub().args(["capabilities", "--json"]).assert().success();
    let v: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    let pairs = v["pairs"].as_array().unwrap();
    assert_eq!(pairs.len(), 10);
    assert!(pairs.contains(&serde_json::json!([2026, 2024])));
    assert!(!pairs.contains(&serde_json::json!([2023, 2023])));
}

#[test]
fn version_prints_name_and_version() {
    let out = stub().arg("--version").assert().success();
    let s = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert_eq!(
        s.lines().next().unwrap(),
        format!("engine-stub {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn content_marker_overrides_name() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("plain.rvt");
    let mut bytes = b"head ".to_vec();
    bytes.extend("Format: 2024".encode_utf16().flat_map(|u| u.to_le_bytes()));
    bytes.extend(b" tail stub-mode=refuse end");
    fs::write(&f, bytes).unwrap();
    let out = stub().arg("probe").arg(&f).arg("--json").assert().success();
    let v: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["year"], 2024);
    assert_eq!(v["targets"], serde_json::json!([2023]));
    stub()
        .arg("downgrade")
        .arg(&f)
        .args(["--to", "2023", "--out"])
        .arg(dir.path().join("o.rvt"))
        .arg("--log")
        .arg(dir.path().join("l.jsonl"))
        .assert()
        .code(3);
}

#[test]
fn name_token_not_shadowed_by_ok() {
    let (a, out, _d) = run_mode("broken_fail.rvt");
    a.code(5);
    assert!(!out.exists());
}

#[test]
fn env_mode_used_when_name_has_no_token() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("look.rvt");
    fs::write(&input, b"x").unwrap();
    stub()
        .env("BACKSAVE_STUB_MODE", "corrupt")
        .arg("downgrade")
        .arg(&input)
        .args(["--to", "2024", "--out"])
        .arg(dir.path().join("o.rvt"))
        .arg("--log")
        .arg(dir.path().join("l.jsonl"))
        .assert()
        .code(4);
}

fn run_downgrade(dir: &Path, marker: &[u8]) -> Command {
    let input = dir.join("in.rvt");
    fs::write(&input, marker).unwrap();
    let mut c = stub();
    c.arg("downgrade")
        .arg(&input)
        .args(["--to", "2024", "--out"])
        .arg(dir.join("out.rvt"))
        .arg("--log")
        .arg(dir.join("log.jsonl"));
    c
}

#[test]
fn no_log_mode_succeeds_without_log() {
    let dir = tempfile::tempdir().unwrap();
    run_downgrade(dir.path(), b"stub-mode=no-log")
        .assert()
        .success();
    assert!(dir.path().join("out.rvt").exists());
    assert!(!dir.path().join("log.jsonl").exists());
}

#[test]
fn probe_hang_mode_hangs_only_probe() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("p.rvt");
    fs::write(&input, b"stub-mode=probe-hang").unwrap();
    stub()
        .arg("probe")
        .arg(&input)
        .arg("--json")
        .timeout(std::time::Duration::from_millis(500))
        .assert()
        .failure();
    run_downgrade(dir.path(), b"stub-mode=probe-hang")
        .assert()
        .success();
}

#[test]
fn hang_mode_writes_pid_file_next_to_out() {
    let dir = tempfile::tempdir().unwrap();
    run_downgrade(dir.path(), b"stub-mode=hang")
        .timeout(std::time::Duration::from_millis(1500))
        .assert()
        .failure();
    let pid = fs::read_to_string(dir.path().join("out.rvt.pid")).unwrap();
    assert!(pid.trim().parse::<u32>().is_ok());
}
