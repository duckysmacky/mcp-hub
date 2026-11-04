use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, bail};
use serde::Deserialize;
use sqlx::postgres::{PgConnectOptions, PgSslMode};
use url::{Host, Url};

/// Everything loaded at startup: sensitive and deployment values from the
/// environment, non-sensitive tuning from `settings.toml`.
pub struct Config {
    pub public_url: PublicUrl,
    pub allow_signup: bool,
    pub bind_addr: SocketAddr,
    pub database: PgConnectOptions,
    pub settings: Settings,
}

impl Config {
    pub fn from_env(settings: Settings) -> anyhow::Result<Self> {
        Ok(Self {
            public_url: required_env("PUBLIC_URL")?.parse()?,
            allow_signup: parse_bool("ALLOW_SIGNUP", &optional_env("ALLOW_SIGNUP"))?,
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

fn parse_bool(name: &str, value: &Option<String>) -> anyhow::Result<bool> {
    match value.as_deref() {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        Some(_) => bail!("{name} must be true or false"),
    }
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

fn host_is_loopback(url: &Url) -> bool {
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
    pub paths: PathSettings,
    pub runtimes: RuntimeSettings,
    pub limits: LimitSettings,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PathSettings {
    pub catalog: PathBuf,
    pub packages: PathBuf,
    pub scratch: PathBuf,
}

impl Default for PathSettings {
    fn default() -> Self {
        Self {
            catalog: "/etc/mcp-hub/catalog.toml".into(),
            packages: "/data/packages".into(),
            scratch: "/tmp/mcp-hub/scratch".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RuntimeSettings {
    pub bun: PathBuf,
    pub node: PathBuf,
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        Self {
            bun: "/usr/local/bin/bun".into(),
            node: "/usr/local/bin/node".into(),
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
    fn settings_file_may_only_be_missing_when_optional() {
        let missing = Path::new("/nonexistent/settings.toml");
        assert!(Settings::load(missing, false).is_ok());
        assert!(Settings::load(missing, true).is_err());
    }
}
