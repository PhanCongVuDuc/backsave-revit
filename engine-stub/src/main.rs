//! Stand-in for the conversion engine, used in development and tests.
//!
//! Behaviour is chosen from the file content first, then the file name, then the
//! `BACKSAVE_STUB_MODE` environment variable, then defaults (year 2026, mode ok).

use clap::{Parser, Subcommand};
use serde_json::json;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::exit;

const MIN_YEAR: u32 = 2023;
const MAX_YEAR: u32 = 2027;
const MODES: [&str; 6] = ["ok", "refuse", "corrupt", "fail", "hang", "crash"];

#[derive(Parser)]
#[command(name = "engine-stub", version, disable_version_flag = true)]
struct Cli {
    /// Print the engine name and version.
    #[arg(long = "version", global = false)]
    version: bool,
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// List supported (from, to) version pairs.
    Capabilities {
        #[arg(long)]
        json: bool,
    },
    /// Report the kind, version and possible targets of a file.
    Probe {
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Downgrade a file to an older version.
    Downgrade {
        input: PathBuf,
        #[arg(long)]
        to: u32,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        log: PathBuf,
    },
}

fn fail(code: i32, msg: &str) -> ! {
    eprintln!("{msg}");
    exit(code);
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

fn utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
}

fn content_year(bytes: &[u8]) -> Option<u32> {
    let key = utf16le("Format: ");
    let pos = find(bytes, &key)? + key.len();
    let digits = bytes.get(pos..pos + 8)?;
    let mut y = 0u32;
    for pair in digits.chunks(2) {
        if pair[1] != 0 || !pair[0].is_ascii_digit() {
            return None;
        }
        y = y * 10 + u32::from(pair[0] - b'0');
    }
    Some(y)
}

fn content_mode(bytes: &[u8]) -> Option<&'static str> {
    let pos = find(bytes, b"stub-mode=")? + b"stub-mode=".len();
    let rest = &bytes[pos..];
    MODES.iter().copied().find(|m| rest.starts_with(m.as_bytes()))
}

fn name_year(name: &str) -> Option<u32> {
    (2022..=2027).find(|y| name.contains(&y.to_string()))
}

fn name_mode(name: &str) -> Option<&'static str> {
    MODES.iter().copied().filter(|m| *m != "ok").find(|m| name.contains(m))
}

struct Selection {
    year: u32,
    mode: &'static str,
}

fn select(path: &Path, bytes: &[u8]) -> Selection {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let env_mode = std::env::var("BACKSAVE_STUB_MODE").ok().and_then(|v| {
        let v = v.to_lowercase();
        MODES.iter().copied().find(|m| *m == v)
    });
    Selection {
        year: content_year(bytes).or_else(|| name_year(&name)).unwrap_or(2026),
        mode: content_mode(bytes)
            .or_else(|| name_mode(&name))
            .or(env_mode)
            .unwrap_or("ok"),
    }
}

fn kind_of(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "rvt" => "rvt",
        "rte" => "rte",
        "rfa" => "rfa",
        "rft" => "rft",
        _ => fail(3, "Unsupported file type"),
    }
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|_| fail(4, "The file could not be read"))
}

fn targets(year: u32) -> Vec<u32> {
    (MIN_YEAR..year).rev().collect()
}

fn check_year(year: u32) {
    if !(MIN_YEAR..=MAX_YEAR).contains(&year) {
        fail(3, &format!("Revit {year} files are not supported"));
    }
}

fn probe(file: &Path) {
    let kind = kind_of(file);
    let bytes = read(file);
    let sel = select(file, &bytes);
    check_year(sel.year);
    println!("{}", json!({"kind": kind, "year": sel.year, "targets": targets(sel.year)}));
}

fn write_log(log: &Path, kind: &str, year: u32, to: u32) -> std::io::Result<()> {
    let mut s = String::new();
    let mut line = |v: serde_json::Value| {
        let _ = writeln!(s, "{v}");
    };
    line(json!({"type":"run","engine":"engine-stub","kind":kind,"from":year,"to":to}));
    let mut id = 1000u64;
    for _ in 0..61 {
        line(json!({"type":"element_dropped","id":id,"class":"CustomElement","reason":"no_rule",
            "cascade_from":null,"rule":null,"method_doc":"stub/custom-element.md","step":3}));
        id += 1;
    }
    for _ in 0..2 {
        line(json!({"type":"element_dropped","id":id,"class":"MEPSystemZoneElementType","reason":"no_rule",
            "cascade_from":null,"rule":null,"method_doc":"stub/mep-zone-type.md","step":5}));
        id += 1;
    }
    for _ in 0..9 {
        line(json!({"type":"element_dropped","id":id,"class":"ElementHeader","reason":"cascade",
            "cascade_from":1000,"rule":"R-cascade-header","method_doc":"stub/cascade.md","step":7}));
        id += 1;
    }
    line(json!({"type":"field_dropped","class":"Wall","field":"StubField","rule":"R-field-1",
        "method_doc":"stub/field.md","step":4}));
    line(json!({"type":"summary","elements_dropped":72,"fields_dropped":1,"steps":year - to}));
    std::fs::write(log, s)
}

fn downgrade(input: &Path, to: u32, out: &Path, log: &Path) {
    let kind = kind_of(input);
    let bytes = read(input);
    let sel = select(input, &bytes);
    match sel.mode {
        "refuse" => fail(3, "Worksharing central files are not supported yet"),
        "corrupt" => fail(4, "The file is corrupt or unreadable"),
        "fail" => fail(5, "The conversion failed"),
        "hang" => loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        },
        "crash" => std::process::abort(),
        _ => {}
    }
    check_year(sel.year);
    if !targets(sel.year).contains(&to) {
        fail(3, &format!("Cannot convert a Revit {} file to {to}", sel.year));
    }
    if std::fs::write(out, &bytes).is_err() || write_log(log, kind, sel.year, to).is_err() {
        let _ = std::fs::remove_file(out);
        let _ = std::fs::remove_file(log);
        fail(5, "Could not write the output");
    }
}

fn main() {
    let cli = Cli::parse();
    if cli.version {
        println!("engine-stub {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    match cli.command {
        Some(Cmd::Capabilities { .. }) => {
            let mut pairs = Vec::new();
            for from in (MIN_YEAR + 1..=MAX_YEAR).rev() {
                for to in targets(from) {
                    pairs.push(json!([from, to]));
                }
            }
            println!("{}", json!({"pairs": pairs}));
        }
        Some(Cmd::Probe { file, .. }) => probe(&file),
        Some(Cmd::Downgrade { input, to, out, log }) => downgrade(&input, to, &out, &log),
        None => fail(2, "missing subcommand"),
    }
}
