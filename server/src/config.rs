//! Server configuration, read from `REVIT_DOWNGRADE_*` environment variables.

use anyhow::{Context, bail};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::str::FromStr;

/// Maximum accepted upload size: 1 GiB.
pub const MAX_BYTES: u64 = 1 << 30;

const GIB: u64 = 1 << 30;
const DEFAULT_DISK_BUDGET_GIB: u64 = 100;

/// An IP network written as `addr` or `addr/prefix`, e.g. `10.0.0.0/8` or `2001:db8::/32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpCidr {
    addr: IpAddr,
    prefix: u8,
}

impl IpCidr {
    /// True when `ip` lies in this network. IPv4-mapped IPv6 addresses count as IPv4.
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip.to_canonical()) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix))
                    .unwrap_or(0);
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix))
                    .unwrap_or(0);
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

impl FromStr for IpCidr {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<IpCidr> {
        let (addr, prefix) = match s.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (s, None),
        };
        let addr = addr
            .trim()
            .parse::<IpAddr>()
            .with_context(|| format!("{s:?} is not an IP address or network"))?
            .to_canonical();
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            None => max,
            Some(p) => p
                .trim()
                .parse::<u8>()
                .ok()
                .filter(|p| *p <= max)
                .with_context(|| format!("{s:?} has an invalid prefix length"))?,
        };
        Ok(IpCidr { addr, prefix })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub engine: PathBuf,
    pub data: PathBuf,
    pub workers: usize,
    pub listen: SocketAddr,
    pub web_dir: Option<PathBuf>,
    pub trust_proxy: bool,
    /// When not empty, `X-Forwarded-For` is honored only for requests from these peers.
    pub trusted_proxies: Vec<IpCidr>,
    pub max_bytes: u64,
    /// Disk space all live jobs may use together, in bytes.
    pub disk_budget: u64,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Config> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Builds the configuration from an arbitrary variable lookup (used by tests).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> anyhow::Result<Config> {
        let non_empty = |k: &str| get(k).filter(|v| !v.is_empty());
        let engine = non_empty("REVIT_DOWNGRADE_ENGINE").context("REVIT_DOWNGRADE_ENGINE is required")?;
        let workers = match non_empty("REVIT_DOWNGRADE_WORKERS") {
            Some(v) => v
                .parse::<usize>()
                .context("REVIT_DOWNGRADE_WORKERS must be a number")?,
            None => 1,
        };
        if workers == 0 {
            bail!("REVIT_DOWNGRADE_WORKERS must be at least 1");
        }
        let listen = non_empty("REVIT_DOWNGRADE_LISTEN")
            .unwrap_or_else(|| "127.0.0.1:8080".to_string())
            .parse()
            .context("REVIT_DOWNGRADE_LISTEN must be host:port")?;
        let trust_proxy = match non_empty("REVIT_DOWNGRADE_TRUST_PROXY") {
            None => false,
            Some(v) => match v.to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => true,
                "0" | "false" | "no" | "off" => false,
                _ => bail!("REVIT_DOWNGRADE_TRUST_PROXY must be true or false"),
            },
        };
        let trusted_proxies = match non_empty("REVIT_DOWNGRADE_TRUSTED_PROXIES") {
            None => Vec::new(),
            Some(v) => v
                .split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::parse)
                .collect::<anyhow::Result<Vec<IpCidr>>>()
                .context(
                    "REVIT_DOWNGRADE_TRUSTED_PROXIES must be a comma-separated list of IPs or CIDRs",
                )?,
        };
        let budget_gib = match non_empty("REVIT_DOWNGRADE_DISK_BUDGET_GIB") {
            None => DEFAULT_DISK_BUDGET_GIB,
            Some(v) => v
                .parse::<u64>()
                .ok()
                .filter(|n| (1..=1 << 20).contains(n))
                .context("REVIT_DOWNGRADE_DISK_BUDGET_GIB must be a whole number of GiB, at least 1")?,
        };
        Ok(Config {
            engine: PathBuf::from(engine),
            data: non_empty("REVIT_DOWNGRADE_DATA").map_or_else(|| PathBuf::from("./data"), PathBuf::from),
            workers,
            listen,
            web_dir: non_empty("REVIT_DOWNGRADE_WEB_DIR").map(PathBuf::from),
            trust_proxy,
            trusted_proxies,
            max_bytes: MAX_BYTES,
            disk_budget: budget_gib * GIB,
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
            Config::from_lookup(lookup(HashMap::from([("REVIT_DOWNGRADE_ENGINE", "/bin/eng")]))).unwrap();
        assert_eq!(c.engine, PathBuf::from("/bin/eng"));
        assert_eq!(c.data, PathBuf::from("./data"));
        assert_eq!(c.workers, 1);
        assert_eq!(c.listen, "127.0.0.1:8080".parse().unwrap());
        assert_eq!(c.web_dir, None);
        assert!(!c.trust_proxy);
        assert!(c.trusted_proxies.is_empty());
        assert_eq!(c.max_bytes, 1_073_741_824);
        assert_eq!(c.disk_budget, 100 * 1_073_741_824);
    }

    #[test]
    fn config_overrides() {
        let c = Config::from_lookup(lookup(HashMap::from([
            ("REVIT_DOWNGRADE_ENGINE", "e"),
            ("REVIT_DOWNGRADE_DATA", "/d"),
            ("REVIT_DOWNGRADE_WORKERS", "3"),
            ("REVIT_DOWNGRADE_LISTEN", "0.0.0.0:9000"),
            ("REVIT_DOWNGRADE_WEB_DIR", "/w"),
            ("REVIT_DOWNGRADE_TRUST_PROXY", "true"),
            ("REVIT_DOWNGRADE_TRUSTED_PROXIES", "172.16.0.0/12, ::1"),
            ("REVIT_DOWNGRADE_DISK_BUDGET_GIB", "7"),
        ])))
        .unwrap();
        assert_eq!(c.data, PathBuf::from("/d"));
        assert_eq!(c.workers, 3);
        assert_eq!(c.listen, "0.0.0.0:9000".parse().unwrap());
        assert_eq!(c.web_dir, Some(PathBuf::from("/w")));
        assert!(c.trust_proxy);
        assert_eq!(
            c.trusted_proxies,
            vec!["172.16.0.0/12".parse().unwrap(), "::1/128".parse().unwrap()]
        );
        assert_eq!(c.disk_budget, 7 * 1_073_741_824);
    }

    #[test]
    fn engine_is_required_and_values_validated() {
        assert!(Config::from_lookup(lookup(HashMap::new())).is_err());
        for (k, v) in [
            ("REVIT_DOWNGRADE_WORKERS", "0"),
            ("REVIT_DOWNGRADE_LISTEN", "nope"),
            ("REVIT_DOWNGRADE_TRUSTED_PROXIES", "10.0.0.0/33"),
            ("REVIT_DOWNGRADE_TRUSTED_PROXIES", "proxy.local"),
            ("REVIT_DOWNGRADE_DISK_BUDGET_GIB", "0"),
            ("REVIT_DOWNGRADE_DISK_BUDGET_GIB", "1.5"),
        ] {
            let bad = HashMap::from([("REVIT_DOWNGRADE_ENGINE", "e"), (k, v)]);
            assert!(Config::from_lookup(lookup(bad)).is_err(), "{k}={v}");
        }
    }

    #[test]
    fn cidr_contains() {
        let net: IpCidr = "172.16.0.0/12".parse().unwrap();
        assert!(net.contains("172.20.1.2".parse().unwrap()));
        assert!(net.contains("::ffff:172.20.1.2".parse().unwrap()));
        assert!(!net.contains("172.32.0.1".parse().unwrap()));
        assert!(!net.contains("2001:db8::1".parse().unwrap()));
        let one: IpCidr = "10.0.0.5".parse().unwrap();
        assert!(one.contains("10.0.0.5".parse().unwrap()));
        assert!(!one.contains("10.0.0.6".parse().unwrap()));
        let all: IpCidr = "0.0.0.0/0".parse().unwrap();
        assert!(all.contains("8.8.8.8".parse().unwrap()));
        let v6: IpCidr = "2001:db8::/32".parse().unwrap();
        assert!(v6.contains("2001:db8:1::1".parse().unwrap()));
        assert!(!v6.contains("2001:db9::1".parse().unwrap()));
    }
}
