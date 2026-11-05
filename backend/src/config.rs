use std::{
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use sqlx::postgres::{PgConnectOptions, PgSslMode};
use url::{Host, Url};

use crate::crypto::KdfParams;

/// Everything loaded at startup: sensitive and deployment values from the
/// environment, non-sensitive tuning from `settings.toml`.
pub struct Config {
    pub public_url: PublicUrl,
    pub bind_addr: SocketAddr,
    pub database: PgConnectOptions,
    pub settings: Settings,
}

impl Config {
    pub fn from_env(settings: Settings) -> anyhow::Result<Self> {
        Ok(Self {
            public_url: required_env("PUBLIC_URL")?.parse()?,
            bind_addr: optional_env("BIND_ADDR")
                .unwrap_or_else(|| "0.0.0.0:8080".to_owned())
                .parse()
                .context("BIND_ADDR must be an address like 0.0.0.0:8080")?,
            database: database_options_from_env()?,
            settings,
        })
    }
}

fn required_env(name: &str) -> anyhow::Result<String> {
    optional_env(name).with_context(|| format!("{name} must be set"))
}

fn optional_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn database_options_from_env() -> anyhow::Result<PgConnectOptions> {
    let ssl_mode =
        PgSslMode::from_str(&optional_env("DB_SSLMODE").unwrap_or_else(|| "require".to_owned()))
            .context("DB_SSLMODE is not a valid Postgres sslmode")?;
    let port = optional_env("DB_PORT")
        .unwrap_or_else(|| "5432".to_owned())
        .parse()
        .context("DB_PORT must be a port number")?;

    Ok(PgConnectOptions::new()
        .host(&required_env("DB_HOST")?)
        .port(port)
        .database(&required_env("DB_NAME")?)
        .username(&required_env("DB_USER")?)
        .password(&required_env("DB_PASSWORD")?)
        .ssl_mode(ssl_mode))
}

/// The hub's public origin: the OAuth issuer and the base of every resource URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicUrl {
    origin: String,
    is_https: bool,
}

impl PublicUrl {
    pub fn as_str(&self) -> &str {
        &self.origin
    }

    pub fn is_https(&self) -> bool {
        self.is_https
    }
}

impl FromStr for PublicUrl {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> anyhow::Result<Self> {
        let url = Url::parse(value).context("PUBLIC_URL must be an absolute URL")?;
        let is_https = url.scheme() == "https";
        if !is_https && !(url.scheme() == "http" && host_is_loopback(&url)) {
            bail!("PUBLIC_URL must be https (http is only allowed for a loopback host)");
        }
        if value.ends_with('/') || url.path() != "/" {
            bail!("PUBLIC_URL must not have a path or trailing slash");
        }
        if url.query().is_some() || url.fragment().is_some() {
            bail!("PUBLIC_URL must not have a query or fragment");
        }
        if !url.username().is_empty() || url.password().is_some() {
            bail!("PUBLIC_URL must not contain credentials");
        }
        Ok(Self {
            origin: url.as_str().trim_end_matches('/').to_owned(),
            is_https,
        })
    }
}

pub fn host_is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain == "localhost",
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

/// Non-sensitive tuning. Every key has a default and unknown keys are rejected,
/// so a credential pasted into the file stops the hub from starting.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub server: ServerSettings,
    pub catalog: CatalogSettings,
    pub paths: PathSettings,
    pub runtimes: RuntimeSettings,
    pub limits: LimitSettings,
    pub http_mode: HttpModeSettings,
    pub network: NetworkSettings,
    pub rate_limits: RateLimitSettings,
    pub audit: AuditSettings,
    pub sessions: SessionSettings,
    pub oauth: OAuthSettings,
    pub password_kdf: PasswordKdfSettings,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerSettings {
    pub signup: SignupMode,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CatalogSettings {
    /// Refuse to start unless `catalog.lock` covers every enabled server.
    pub require_lock: bool,
    /// Lets every account define and run its own servers: code of their choice under the
    /// hub's UID. Only for hubs whose users are all trusted like the operator (SECURITY.md T11).
    pub allow_custom_servers: bool,
    pub custom_install_timeout_secs: u64,
}

impl Default for CatalogSettings {
    fn default() -> Self {
        Self {
            require_lock: true,
            allow_custom_servers: false,
            custom_install_timeout_secs: 300,
        }
    }
}

/// Who may create an account. Closed unless the operator opens it.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SignupMode {
    Enabled,
    #[default]
    Disabled,
    InviteOnly,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OAuthSettings {
    pub access_token_ttl_secs: u64,
    pub refresh_token_ttl_secs: u64,
    pub auth_code_ttl_secs: u64,
}

impl Default for OAuthSettings {
    fn default() -> Self {
        Self {
            access_token_ttl_secs: 3600,
            refresh_token_ttl_secs: 2_592_000,
            auth_code_ttl_secs: 60,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionSettings {
    pub web_session_ttl_secs: u64,
}

impl Default for SessionSettings {
    fn default() -> Self {
        Self {
            web_session_ttl_secs: 2_592_000,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PasswordKdfSettings {
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

impl Default for PasswordKdfSettings {
    fn default() -> Self {
        Self {
            memory_kib: 65536,
            iterations: 3,
            parallelism: 1,
        }
    }
}

impl PasswordKdfSettings {
    pub fn params(&self) -> KdfParams {
        KdfParams {
            memory_kib: self.memory_kib,
            iterations: self.iterations,
            parallelism: self.parallelism,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HttpModeSettings {
    /// Inclusive range of loopback ports for upstream processes.
    pub port_range: [u16; 2],
}

impl Default for HttpModeSettings {
    fn default() -> Self {
        Self {
            port_range: [41000, 41999],
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkSettings {
    /// The host's public IPs: `net_guard` allows them only on ports 443 and 80.
    pub self_addresses: Vec<IpAddr>,
    pub allow_http_urls: bool,
    /// Peers whose `X-Forwarded-For` is believed. Everyone else's is ignored.
    pub trusted_proxies: Vec<Cidr>,
}

/// An address range such as `172.16.0.0/12`. A bare address is a range of one.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(try_from = "String")]
pub struct Cidr {
    network: IpAddr,
    prefix_bits: u8,
}

impl TryFrom<String> for Cidr {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let invalid = || format!("{text} is not an address range like 172.16.0.0/12");
        let (address, prefix) = match text.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix)),
            None => (text.as_str(), None),
        };
        let network: IpAddr = address.parse().map_err(|_| invalid())?;
        let max_bits = if network.is_ipv4() { 32 } else { 128 };
        let prefix_bits = match prefix {
            Some(prefix) => prefix.parse().map_err(|_| invalid())?,
            None => max_bits,
        };
        if prefix_bits > max_bits {
            return Err(invalid());
        }
        Ok(Self {
            network,
            prefix_bits,
        })
    }
}

impl Cidr {
    pub fn contains(&self, address: IpAddr) -> bool {
        match (self.network, address.to_canonical()) {
            (IpAddr::V4(network), IpAddr::V4(address)) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix_bits))
                    .unwrap_or(0);
                u32::from(network) & mask == u32::from(address) & mask
            }
            (IpAddr::V6(network), IpAddr::V6(address)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix_bits))
                    .unwrap_or(0);
                u128::from(network) & mask == u128::from(address) & mask
            }
            _ => false,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuditSettings {
    pub retention_days: u32,
}

impl Default for AuditSettings {
    fn default() -> Self {
        Self { retention_days: 90 }
    }
}

/// Requests per client address per minute, except where a key says otherwise.
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RateLimitSettings {
    pub login: u32,
    /// Across all addresses, against guessing one account's password from many.
    pub login_per_username: u32,
    pub signup: u32,
    pub password_change: u32,
    pub oauth_register: u32,
    pub oauth_authorize: u32,
    pub oauth_token: u32,
    pub oauth_revoke: u32,
}

impl Default for RateLimitSettings {
    fn default() -> Self {
        Self {
            login: 10,
            login_per_username: 10,
            signup: 5,
            password_change: 5,
            oauth_register: 10,
            oauth_authorize: 30,
            oauth_token: 60,
            oauth_revoke: 60,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PathSettings {
    pub catalog: PathBuf,
    pub packages: PathBuf,
    pub scratch: PathBuf,
    #[serde(rename = "static")]
    pub r#static: PathBuf,
}

impl Default for PathSettings {
    fn default() -> Self {
        Self {
            catalog: "/etc/mcp-hub/catalog.toml".into(),
            packages: "/data/packages".into(),
            scratch: "/tmp/mcp-hub/scratch".into(),
            r#static: "/usr/share/mcp-hub/static".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RuntimeSettings {
    pub bun: PathBuf,
    pub node: PathBuf,
    pub uv: PathBuf,
    pub pip: PathBuf,
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        Self {
            bun: "/usr/local/bin/bun".into(),
            node: "/usr/local/bin/node".into(),
            uv: "/usr/local/bin/uv".into(),
            pip: "/usr/local/bin/pip".into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LimitSettings {
    pub max_processes_per_user: usize,
    pub max_sessions_per_instance: usize,
    pub max_processes_total: usize,
    pub max_spawns_per_user_per_minute: usize,
    pub max_pending_requests_per_session: usize,
    pub max_instances_per_user: i64,
    /// Each password hash takes `password_kdf.memory_kib`, so this bounds memory under a flood.
    pub max_concurrent_password_hashes: usize,
    pub idle_timeout_secs: u64,
    pub initialize_timeout_secs: u64,
    pub request_timeout_secs: u64,
    pub upstream_start_timeout_secs: u64,
    pub max_body_bytes: usize,
    pub max_line_bytes: usize,
    pub output_tail_bytes: usize,
}

impl Default for LimitSettings {
    fn default() -> Self {
        Self {
            max_processes_per_user: 6,
            max_sessions_per_instance: 3,
            max_processes_total: 50,
            max_spawns_per_user_per_minute: 10,
            max_pending_requests_per_session: 32,
            max_instances_per_user: 20,
            max_concurrent_password_hashes: 4,
            idle_timeout_secs: 600,
            initialize_timeout_secs: 30,
            request_timeout_secs: 300,
            upstream_start_timeout_secs: 15,
            max_body_bytes: 4_194_304,
            max_line_bytes: 8_388_608,
            output_tail_bytes: 65_536,
        }
    }
}

impl Settings {
    /// Loads `path`. A missing file means defaults only when `required` is false.
    pub fn load(path: &Path, required: bool) -> anyhow::Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !required => {
                return Ok(Self::default());
            }
            Err(error) => {
                return Err(error).with_context(|| format!("reading {}", path.display()));
            }
        };
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_url_accepts_only_clean_origins() {
        for valid in [
            "https://mcp.example.com",
            "https://mcp.example.com:8443",
            "http://localhost:8080",
            "http://127.0.0.1:8080",
            "http://[::1]:8080",
        ] {
            let parsed: PublicUrl = valid.parse().unwrap();
            assert_eq!(parsed.as_str(), valid);
        }

        for invalid in [
            "mcp.example.com",
            "http://mcp.example.com",
            "https://mcp.example.com/",
            "https://mcp.example.com/hub",
            "https://mcp.example.com?a=1",
            "https://mcp.example.com#top",
            "https://user:pass@mcp.example.com",
            "ftp://localhost",
        ] {
            assert!(invalid.parse::<PublicUrl>().is_err(), "{invalid}");
        }
    }

    #[test]
    fn address_ranges_match_what_they_should() {
        let cidr = |text: &str| Cidr::try_from(text.to_owned()).unwrap();
        let address = |text: &str| text.parse::<IpAddr>().unwrap();

        assert!(cidr("172.16.0.0/12").contains(address("172.31.255.1")));
        assert!(!cidr("172.16.0.0/12").contains(address("172.32.0.1")));
        assert!(cidr("10.0.0.1").contains(address("10.0.0.1")));
        assert!(!cidr("10.0.0.1").contains(address("10.0.0.2")));
        assert!(cidr("0.0.0.0/0").contains(address("8.8.8.8")));
        assert!(cidr("fd00::/8").contains(address("fd12::1")));
        assert!(!cidr("fd00::/8").contains(address("fe80::1")));
        assert!(cidr("::/0").contains(address("2001:db8::1")));
        // A v4 peer on a dual-stack socket arrives as a mapped v6 address.
        assert!(cidr("10.0.0.0/8").contains(address("::ffff:10.1.2.3")));
        assert!(!cidr("10.0.0.0/8").contains(address("2001:db8::1")));

        for invalid in ["", "nonsense", "10.0.0.0/33", "fd00::/129", "10.0.0.0/x"] {
            assert!(Cidr::try_from(invalid.to_owned()).is_err(), "{invalid}");
        }
    }

    #[test]
    fn settings_reject_unknown_keys() {
        assert!(toml::from_str::<Settings>("db_password = \"x\"").is_err());
        assert!(toml::from_str::<Settings>("[paths]\nsecret = \"x\"").is_err());
    }

    #[test]
    fn dev_settings_file_is_valid() {
        let settings: Settings = toml::from_str(include_str!("../../settings.dev.toml")).unwrap();
        assert_eq!(settings.runtimes.bun, Path::new("/usr/bin/bun"));
    }

    #[test]
    fn example_settings_file_lists_valid_keys() {
        let settings: Settings =
            toml::from_str(include_str!("../../settings.example.toml")).unwrap();
        assert_eq!(settings.server.signup, SignupMode::Disabled);
    }

    #[test]
    fn signup_is_closed_by_default_and_has_three_modes() {
        assert_eq!(Settings::default().server.signup, SignupMode::Disabled);
        for (text, mode) in [
            ("enabled", SignupMode::Enabled),
            ("disabled", SignupMode::Disabled),
            ("invite-only", SignupMode::InviteOnly),
        ] {
            let settings: Settings =
                toml::from_str(&format!("[server]\nsignup = \"{text}\"")).unwrap();
            assert_eq!(settings.server.signup, mode);
        }
        assert!(toml::from_str::<Settings>("[server]\nsignup = \"maybe\"").is_err());
    }

    #[test]
    fn the_catalog_lock_is_required_unless_turned_off() {
        assert!(Settings::default().catalog.require_lock);
        let settings: Settings = toml::from_str("[catalog]\nrequire_lock = false").unwrap();
        assert!(!settings.catalog.require_lock);
        assert!(toml::from_str::<Settings>("[catalog]\nnonsense = true").is_err());
    }

    #[test]
    fn custom_servers_are_off_by_default() {
        assert!(!Settings::default().catalog.allow_custom_servers);
    }

    #[test]
    fn settings_file_may_only_be_missing_when_optional() {
        let missing = Path::new("/nonexistent/settings.toml");
        assert!(Settings::load(missing, false).is_ok());
        assert!(Settings::load(missing, true).is_err());
    }
}
