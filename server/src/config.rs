//! Server configuration, read from `BACKSAVE_*` environment variables.

use anyhow::{Context, bail};
use std::net::SocketAddr;
use std::path::PathBuf;

/// Maximum accepted upload size: 1 GiB.
pub const MAX_BYTES: u64 = 1 << 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub engine: PathBuf,
    pub data: PathBuf,
    pub workers: usize,
    pub listen: SocketAddr,
    pub web_dir: Option<PathBuf>,
    pub trust_proxy: bool,
    pub max_bytes: u64,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Config> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Builds the configuration from an arbitrary variable lookup (used by tests).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> anyhow::Result<Config> {
        let non_empty = |k: &str| get(k).filter(|v| !v.is_empty());
        let engine = non_empty("BACKSAVE_ENGINE").context("BACKSAVE_ENGINE is required")?;
        let workers = match non_empty("BACKSAVE_WORKERS") {
            Some(v) => v
                .parse::<usize>()
                .context("BACKSAVE_WORKERS must be a number")?,
            None => 1,
        };
        if workers == 0 {
            bail!("BACKSAVE_WORKERS must be at least 1");
        }
        let listen = non_empty("BACKSAVE_LISTEN")
            .unwrap_or_else(|| "127.0.0.1:8080".to_string())
            .parse()
            .context("BACKSAVE_LISTEN must be host:port")?;
        let trust_proxy = match non_empty("BACKSAVE_TRUST_PROXY") {
            None => false,
            Some(v) => match v.to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => true,
                "0" | "false" | "no" | "off" => false,
                _ => bail!("BACKSAVE_TRUST_PROXY must be true or false"),
            },
        };
        Ok(Config {
            engine: PathBuf::from(engine),
            data: non_empty("BACKSAVE_DATA").map_or_else(|| PathBuf::from("./data"), PathBuf::from),
            workers,
            listen,
            web_dir: non_empty("BACKSAVE_WEB_DIR").map(PathBuf::from),
            trust_proxy,
            max_bytes: MAX_BYTES,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(m: HashMap<&'static str, &'static str>) -> impl Fn(&str) -> Option<String> {
        move |k| m.get(k).map(|v| v.to_string())
    }

    #[test]
    fn config_defaults() {
        let c =
            Config::from_lookup(lookup(HashMap::from([("BACKSAVE_ENGINE", "/bin/eng")]))).unwrap();
        assert_eq!(c.engine, PathBuf::from("/bin/eng"));
        assert_eq!(c.data, PathBuf::from("./data"));
        assert_eq!(c.workers, 1);
        assert_eq!(c.listen, "127.0.0.1:8080".parse().unwrap());
        assert_eq!(c.web_dir, None);
        assert!(!c.trust_proxy);
        assert_eq!(c.max_bytes, 1_073_741_824);
    }

    #[test]
    fn config_overrides() {
        let c = Config::from_lookup(lookup(HashMap::from([
            ("BACKSAVE_ENGINE", "e"),
            ("BACKSAVE_DATA", "/d"),
            ("BACKSAVE_WORKERS", "3"),
            ("BACKSAVE_LISTEN", "0.0.0.0:9000"),
            ("BACKSAVE_WEB_DIR", "/w"),
            ("BACKSAVE_TRUST_PROXY", "true"),
        ])))
        .unwrap();
        assert_eq!(c.data, PathBuf::from("/d"));
        assert_eq!(c.workers, 3);
        assert_eq!(c.listen, "0.0.0.0:9000".parse().unwrap());
        assert_eq!(c.web_dir, Some(PathBuf::from("/w")));
        assert!(c.trust_proxy);
    }

    #[test]
    fn engine_is_required_and_values_validated() {
        assert!(Config::from_lookup(lookup(HashMap::new())).is_err());
        let bad = HashMap::from([("BACKSAVE_ENGINE", "e"), ("BACKSAVE_WORKERS", "0")]);
        assert!(Config::from_lookup(lookup(bad)).is_err());
        let bad = HashMap::from([("BACKSAVE_ENGINE", "e"), ("BACKSAVE_LISTEN", "nope")]);
        assert!(Config::from_lookup(lookup(bad)).is_err());
    }
}
