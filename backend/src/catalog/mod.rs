//! The operator's list of allowed MCP servers. Users can only pick from it.

pub mod install;

use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

use axum::http::HeaderName;
use serde::{Deserialize, Serialize};

const FORBIDDEN_HEADERS: [&str; 4] = [
    "host",
    "content-length",
    "mcp-session-id",
    "mcp-protocol-version",
];
const DENIED_ENV_PREFIXES: [&str; 8] = [
    "LD_", "DYLD_", "NODE_", "BUN_", "NPM_", "PYTHON", "UV_", "PIP_",
];
const DENIED_ENV_NAMES: [&str; 7] = ["PATH", "HOME", "SHELL", "TMPDIR", "IFS", "ENV", "BASH_ENV"];

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("reading catalog {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("parsing catalog: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("server {server}: {reason}")]
    Invalid { server: String, reason: String },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServerMode {
    Stdio,
    Http,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    Bun,
    Node,
}

impl RuntimeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bun => "bun",
            Self::Node => "node",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpSettings {
    #[serde(default)]
    pub bin_args: Vec<String>,
    pub port_env: String,
    #[serde(default = "default_mcp_path")]
    pub path: String,
    /// Header name to template, where `{VAR}` is replaced by that user's value.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

fn default_mcp_path() -> String {
    "/mcp".to_owned()
}

impl HttpSettings {
    /// Env vars that are only ever sent as request headers, never put in process env.
    pub fn header_vars(&self) -> HashSet<&str> {
        self.headers
            .values()
            .flat_map(|template| template_vars(template).unwrap_or_default())
            .collect()
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerTemplate {
    pub id: String,
    pub mode: ServerMode,
    pub runtime: RuntimeKind,
    pub package: String,
    pub bin: Option<String>,
    pub env_vars: Vec<String>,
    pub http: Option<HttpSettings>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogFile {
    #[serde(default)]
    server: Vec<ServerTemplate>,
}

#[derive(Clone, Debug)]
pub struct Catalog {
    servers: BTreeMap<String, ServerTemplate>,
}

impl Catalog {
    pub fn load(path: &Path) -> Result<Self, CatalogError> {
        let text = std::fs::read_to_string(path).map_err(|source| CatalogError::Read {
            path: path.to_owned(),
            source,
        })?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, CatalogError> {
        let file: CatalogFile = toml::from_str(text)?;
        let mut servers = BTreeMap::new();
        for server in file.server {
            validate_server(&server).map_err(|reason| CatalogError::Invalid {
                server: server.id.clone(),
                reason,
            })?;
            if servers.contains_key(&server.id) {
                return Err(CatalogError::Invalid {
                    server: server.id,
                    reason: "duplicate id".to_owned(),
                });
            }
            servers.insert(server.id.clone(), server);
        }
        Ok(Self { servers })
    }

    pub fn get(&self, id: &str) -> Option<&ServerTemplate> {
        self.servers.get(id)
    }

    pub fn servers(&self) -> impl Iterator<Item = &ServerTemplate> {
        self.servers.values()
    }
}

fn validate_server(server: &ServerTemplate) -> Result<(), String> {
    if !is_valid_server_id(&server.id) {
        return Err("id must match ^[a-z0-9][a-z0-9-]{0,31}$".to_owned());
    }
    split_package_spec(&server.package)?;

    let mut seen = HashSet::new();
    for var in &server.env_vars {
        validate_env_name(var)?;
        if !seen.insert(var.as_str()) {
            return Err(format!("env var {var} is listed twice"));
        }
    }

    match (server.mode, &server.http) {
        (ServerMode::Stdio, None) => Ok(()),
        (ServerMode::Stdio, Some(_)) => {
            Err("[server.http] is only valid with mode = \"http\"".to_owned())
        }
        (ServerMode::Http, None) => Err("mode = \"http\" needs a [server.http] table".to_owned()),
        (ServerMode::Http, Some(http)) => validate_http(server, http),
    }
}

fn validate_http(server: &ServerTemplate, http: &HttpSettings) -> Result<(), String> {
    validate_env_name(&http.port_env)?;
    if server.env_vars.contains(&http.port_env) {
        return Err(format!(
            "port_env {} must not appear in env_vars",
            http.port_env
        ));
    }
    if !http.path.starts_with('/') {
        return Err("http.path must start with /".to_owned());
    }
    for (name, template) in &http.headers {
        let header = HeaderName::try_from(name.as_str())
            .map_err(|_| format!("invalid header name {name}"))?;
        if FORBIDDEN_HEADERS.contains(&header.as_str()) {
            return Err(format!("header {name} is managed by the hub"));
        }
        for var in template_vars(template)? {
            if !server.env_vars.iter().any(|allowed| allowed == var) {
                return Err(format!(
                    "header {name} uses {{{var}}}, which is not in env_vars"
                ));
            }
        }
    }
    Ok(())
}

fn is_valid_server_id(id: &str) -> bool {
    let mut chars = id.chars();
    id.len() <= 32
        && chars
            .next()
            .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars.all(|next| next.is_ascii_lowercase() || next.is_ascii_digit() || next == '-')
}

/// Checks a name against `^[A-Za-z_][A-Za-z0-9_]{0,127}$` and the denylist (SECURITY.md T1).
fn validate_env_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let well_formed = name.len() <= 128
        && chars
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|next| next.is_ascii_alphanumeric() || next == '_');
    if !well_formed {
        return Err(format!("invalid env var name {name}"));
    }

    let upper = name.to_ascii_uppercase();
    if DENIED_ENV_NAMES.contains(&name)
        || DENIED_ENV_PREFIXES
            .iter()
            .any(|prefix| upper.starts_with(prefix))
    {
        return Err(format!("env var {name} is on the denylist"));
    }
    Ok(())
}

/// Splits `name@exact-version`. Ranges, tags, and non-registry specs are rejected,
/// so that only pinned packages are ever installed.
pub fn split_package_spec(spec: &str) -> Result<(&str, &str), String> {
    let separator = spec
        .rfind('@')
        .filter(|&index| index > 0)
        .ok_or_else(|| format!("package {spec} must be name@exact-version"))?;
    let (name, version) = (&spec[..separator], &spec[separator + 1..]);
    if !is_valid_package_name(name) {
        return Err(format!("invalid package name {name}"));
    }
    if !is_exact_version(version) {
        return Err(format!(
            "package {spec} must pin an exact version like 1.2.3"
        ));
    }
    Ok((name, version))
}

fn is_valid_package_name(name: &str) -> bool {
    let is_part = |part: &str| {
        !part.is_empty()
            && !part.starts_with('.')
            && part
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-._~".contains(c))
    };
    match name.strip_prefix('@') {
        Some(scoped) => scoped
            .split_once('/')
            .is_some_and(|(scope, package)| is_part(scope) && is_part(package)),
        None => is_part(name),
    }
}

fn is_exact_version(version: &str) -> bool {
    let (core, suffix) = match version.split_once(['-', '+']) {
        Some((core, suffix)) => (core, Some(suffix)),
        None => (version, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
        && suffix.is_none_or(|text| {
            !text.is_empty()
                && text
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-.+".contains(c))
        })
}

/// Returns the `{VAR}` names used by a header template.
pub fn template_vars(template: &str) -> Result<Vec<&str>, String> {
    let mut vars = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        let end = after
            .find('}')
            .ok_or_else(|| format!("unclosed {{ in header template {template}"))?;
        vars.push(&after[..end]);
        rest = &after[end + 1..];
    }
    if rest.contains('}') {
        return Err(format!("unmatched }} in header template {template}"));
    }
    Ok(vars)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STDIO: &str = r#"
        [[server]]
        id = "taiga-password"
        mode = "stdio"
        runtime = "bun"
        package = "@madebyclowd/taiga-mcp-server@1.0.2"
        bin = "taiga-mcp-server"
        env_vars = ["TAIGA_BASE_URL", "TAIGA_PASSWORD"]
    "#;

    fn http_catalog(env_vars: &str, port_env: &str, headers: &str) -> String {
        format!(
            r#"
            [[server]]
            id = "taiga"
            mode = "http"
            runtime = "bun"
            package = "taiga@1.0.2"
            env_vars = {env_vars}

            [server.http]
            port_env = "{port_env}"
            headers = {headers}
            "#
        )
    }

    fn with_env_var(name: &str) -> String {
        STDIO.replace("TAIGA_PASSWORD", name)
    }

    #[test]
    fn example_catalog_is_valid() {
        let catalog = Catalog::parse(include_str!("../../../catalog.example.toml")).unwrap();
        assert_eq!(catalog.get("taiga").unwrap().mode, ServerMode::Http);
        assert_eq!(catalog.get("firefly").unwrap().mode, ServerMode::Stdio);
    }

    #[test]
    fn header_vars_are_separated_from_process_env() {
        let catalog = Catalog::parse(include_str!("../../../catalog.example.toml")).unwrap();
        let http = catalog.get("taiga").unwrap().http.as_ref().unwrap();
        assert_eq!(http.header_vars(), HashSet::from(["TAIGA_TOKEN"]));
        assert_eq!(http.path, "/mcp");
    }

    #[test]
    fn dangerous_env_names_are_rejected() {
        for name in [
            "LD_PRELOAD",
            "ld_preload",
            "DYLD_INSERT_LIBRARIES",
            "NODE_OPTIONS",
            "node_options",
            "BUN_OPTIONS",
            "NPM_TOKEN",
            "npm_config_registry",
            "PYTHONPATH",
            "UV_INDEX",
            "PIP_INDEX_URL",
            "PATH",
            "HOME",
            "SHELL",
            "TMPDIR",
            "IFS",
            "ENV",
            "BASH_ENV",
            "1BAD",
            "HAS-DASH",
            "",
        ] {
            assert!(Catalog::parse(&with_env_var(name)).is_err(), "{name}");
        }
        let too_long = "A".repeat(129);
        assert!(Catalog::parse(&with_env_var(&too_long)).is_err());
        assert!(Catalog::parse(&with_env_var(&"A".repeat(128))).is_ok());
    }

    #[test]
    fn only_exact_versions_are_accepted() {
        for spec in [
            "a@1.2.3",
            "@scope/a@1.2.3",
            "a@1.2.3-rc.1",
            "a@1.2.3+build5",
        ] {
            assert!(split_package_spec(spec).is_ok(), "{spec}");
        }
        assert_eq!(
            split_package_spec("@scope/a@1.2.3").unwrap(),
            ("@scope/a", "1.2.3")
        );
        for spec in [
            "a",
            "@scope/a",
            "a@latest",
            "a@^1.2.3",
            "a@~1.2.3",
            "a@1.x",
            "a@1.2",
            "a@*",
            "a@>=1.0.0",
            "a@git+https://example.com/a.git",
            "git+https://example.com/a.git",
            "a@file:../evil",
            "../evil@1.2.3",
            "@scope@1.2.3",
        ] {
            assert!(split_package_spec(spec).is_err(), "{spec}");
        }
    }

    #[test]
    fn http_table_must_match_the_mode() {
        let stdio_with_http = format!("{STDIO}\n[server.http]\nport_env = \"PORT\"\n");
        assert!(Catalog::parse(&stdio_with_http).is_err());
        assert!(Catalog::parse(&STDIO.replace("\"stdio\"", "\"http\"")).is_err());
    }

    #[test]
    fn http_rules_are_enforced() {
        let valid = http_catalog(
            r#"["TOKEN"]"#,
            "PORT",
            r#"{ Authorization = "Bearer {TOKEN}" }"#,
        );
        assert!(Catalog::parse(&valid).is_ok());

        let port_in_env_vars = http_catalog(r#"["PORT"]"#, "PORT", "{}");
        assert!(Catalog::parse(&port_in_env_vars).is_err());

        let unknown_var = http_catalog(r#"["TOKEN"]"#, "PORT", r#"{ X-A = "{OTHER}" }"#);
        assert!(Catalog::parse(&unknown_var).is_err());

        let unclosed = http_catalog(r#"["TOKEN"]"#, "PORT", r#"{ X-A = "{TOKEN" }"#);
        assert!(Catalog::parse(&unclosed).is_err());

        for header in [
            "Host",
            "Content-Length",
            "Mcp-Session-Id",
            "MCP-Protocol-Version",
        ] {
            let forbidden =
                http_catalog(r#"["TOKEN"]"#, "PORT", &format!(r#"{{ {header} = "x" }}"#));
            assert!(Catalog::parse(&forbidden).is_err(), "{header}");
        }

        let bad_name = http_catalog(r#"["TOKEN"]"#, "PORT", r#"{ "Bad Header" = "x" }"#);
        assert!(Catalog::parse(&bad_name).is_err());

        let denied_port = http_catalog(r#"["TOKEN"]"#, "NODE_PORT", "{}");
        assert!(Catalog::parse(&denied_port).is_err());
    }

    #[test]
    fn ids_and_duplicates_are_checked() {
        assert!(Catalog::parse(&STDIO.replace("taiga-password", "Bad_Id")).is_err());
        assert!(Catalog::parse(&format!("{STDIO}\n{STDIO}")).is_err());
        assert!(Catalog::parse(&STDIO.replace("bin =", "command =")).is_err());
    }
}
