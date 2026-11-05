//! The operator's list of allowed MCP servers. Users can only pick from it.

pub mod install;
pub mod lock;

use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

use axum::http::HeaderName;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

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
    Uv,
    Pip,
    /// A ready-made executable: `package` is its path, or a name to find on `PATH`.
    Binary,
}

impl RuntimeKind {
    pub fn is_python(self) -> bool {
        matches!(self, Self::Uv | Self::Pip)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpSettings {
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

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EnvType {
    #[default]
    String,
    Number,
    Boolean,
    Enum,
}

/// One env var a user may set for a server. Sent as is to the UI through `/api/catalog`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnvSpec {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "type")]
    pub kind: EnvType,
    #[serde(default)]
    pub enum_values: Vec<String>,
    /// Secret unless the operator says otherwise: a forgotten flag must not expose a value.
    #[serde(default = "default_true")]
    pub secret: bool,
    #[serde(default)]
    pub required: bool,
    /// Vars that can't be set together with this one.
    #[serde(default)]
    pub conflicts: Vec<String>,
}

fn default_true() -> bool {
    true
}

impl EnvSpec {
    /// Checks a value against the declared type. The message never contains the value.
    pub fn validate_value(&self, value: &str) -> Result<(), String> {
        let name = &self.name;
        if self.required && value.is_empty() {
            return Err(format!("{name} is required and can't be empty"));
        }
        match self.kind {
            EnvType::String => Ok(()),
            EnvType::Number if value.parse::<f64>().is_ok_and(f64::is_finite) => Ok(()),
            EnvType::Number => Err(format!("{name} must be a number")),
            EnvType::Boolean if value == "true" || value == "false" => Ok(()),
            EnvType::Boolean => Err(format!("{name} must be true or false")),
            EnvType::Enum if self.enum_values.iter().any(|allowed| allowed == value) => Ok(()),
            EnvType::Enum => Err(format!(
                "{name} must be one of: {}",
                self.enum_values.join(", ")
            )),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerTemplate {
    pub id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub mode: ServerMode,
    pub runtime: RuntimeKind,
    pub package: String,
    pub bin: Option<String>,
    #[serde(default)]
    pub bin_args: Vec<String>,
    #[serde(default)]
    pub env: Vec<EnvSpec>,
    pub http: Option<HttpSettings>,
}

impl ServerTemplate {
    /// The package's name and exact version, by the rules of its runtime's registry.
    pub fn package_parts(&self) -> Result<(&str, &str), String> {
        if self.runtime.is_python() {
            split_python_package_spec(&self.package)
        } else {
            split_package_spec(&self.package)
        }
    }

    pub fn env_spec(&self, name: &str) -> Option<&EnvSpec> {
        self.env.iter().find(|spec| spec.name == name)
    }

    /// Required vars that still need a value. Setting a conflicting var satisfies a
    /// required one: they are alternatives, such as a token or a username and password.
    pub fn missing_required(&self, set_vars: &HashSet<&str>) -> Vec<&str> {
        self.env
            .iter()
            .filter(|spec| spec.required && !set_vars.contains(spec.name.as_str()))
            .filter(|spec| {
                !spec
                    .conflicts
                    .iter()
                    .any(|other| set_vars.contains(other.as_str()))
            })
            .map(|spec| spec.name.as_str())
            .collect()
    }
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
    if server.runtime == RuntimeKind::Binary {
        validate_binary_package(server)?;
    } else {
        server.package_parts()?;
    }
    if server.runtime.is_python() {
        // Python packages have no manifest to pick a default entry point from.
        let bin = server
            .bin
            .as_deref()
            .ok_or("Python servers must set `bin`")?;
        if bin.is_empty() || bin.contains('/') || bin.starts_with('.') {
            return Err(format!("invalid bin name {bin}"));
        }
    }

    let mut seen = HashSet::new();
    for spec in &server.env {
        validate_env_name(&spec.name)?;
        if !seen.insert(spec.name.as_str()) {
            return Err(format!("env var {} is listed twice", spec.name));
        }
        validate_env_spec(server, spec)?;
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

/// A binary is an absolute path or a bare name. A relative path would depend on the
/// hub's working directory, and `..` would hide where the file really is.
fn validate_binary_package(server: &ServerTemplate) -> Result<(), String> {
    if server.bin.is_some() {
        return Err("binary servers must not set `bin`: `package` is the file to run".to_owned());
    }
    let package = &server.package;
    let is_absolute = package.starts_with('/');
    let is_bare_name = !package.is_empty() && !package.contains('/') && !package.starts_with('.');
    let has_parent = package.split('/').any(|part| part == "..");
    if package.contains('\0') || has_parent || !(is_absolute || is_bare_name) {
        return Err(format!(
            "package {package} must be an absolute path or a name to find on PATH"
        ));
    }
    Ok(())
}

fn validate_env_spec(server: &ServerTemplate, spec: &EnvSpec) -> Result<(), String> {
    let name = &spec.name;
    match (spec.kind, spec.enum_values.is_empty()) {
        (EnvType::Enum, true) => return Err(format!("enum var {name} needs enum_values")),
        (EnvType::Enum, false) | (_, true) => {}
        (_, false) => return Err(format!("{name} has enum_values but isn't an enum")),
    }
    for other in &spec.conflicts {
        if other == name {
            return Err(format!("{name} conflicts with itself"));
        }
        let other_spec = server
            .env_spec(other)
            .ok_or_else(|| format!("{name} conflicts with unknown var {other}"))?;
        if !other_spec.conflicts.contains(name) {
            return Err(format!("{other} must also list {name} in its conflicts"));
        }
    }
    Ok(())
}

fn validate_http(server: &ServerTemplate, http: &HttpSettings) -> Result<(), String> {
    validate_env_name(&http.port_env)?;
    if server.env_spec(&http.port_env).is_some() {
        return Err(format!(
            "port_env {} must not be a user-settable env var",
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
            if server.env_spec(var).is_none() {
                return Err(format!("header {name} uses {{{var}}}, which is not in env"));
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

/// Splits `name@exact-version` for PyPI. Only a plain version passes (no `==`, ranges or
/// URLs), and the leading alphanumeric keeps the name from being read as an option.
pub fn split_python_package_spec(spec: &str) -> Result<(&str, &str), String> {
    let (name, version) = spec
        .rsplit_once('@')
        .ok_or_else(|| format!("package {spec} must be name@exact-version"))?;
    let name_is_valid = name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    if !name_is_valid {
        return Err(format!("invalid package name {name}"));
    }
    let version_is_exact = version.starts_with(|c: char| c.is_ascii_digit())
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.');
    if !version_is_exact {
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

/// Fills a header template. `None` if any of its vars has no value.
pub fn render_template(
    template: &str,
    values: &[(String, Zeroizing<String>)],
) -> Option<Zeroizing<String>> {
    let mut rendered = Zeroizing::new(String::with_capacity(template.len()));
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let (literal, after_brace) = (&rest[..start], &rest[start + 1..]);
        let end = after_brace.find('}')?;
        let var = &after_brace[..end];
        let (_, value) = values.iter().find(|(name, _)| name == var)?;
        rendered.push_str(literal);
        rendered.push_str(value);
        rest = &after_brace[end + 1..];
    }
    rendered.push_str(rest);
    Some(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_header_templates() {
        let values = [("TOKEN".to_owned(), Zeroizing::new("abc".to_owned()))];
        assert_eq!(
            render_template("Bearer {TOKEN}", &values).unwrap().as_str(),
            "Bearer abc"
        );
        assert_eq!(
            render_template("static", &values).unwrap().as_str(),
            "static"
        );
        assert_eq!(
            render_template("{TOKEN}-{TOKEN}", &values)
                .unwrap()
                .as_str(),
            "abc-abc"
        );
        assert!(render_template("Bearer {MISSING}", &values).is_none());
    }

    const STDIO: &str = r#"
        [[server]]
        id = "taiga-password"
        mode = "stdio"
        runtime = "bun"
        package = "@madebyclowd/taiga-mcp-server@1.0.2"
        bin = "taiga-mcp-server"

        [[server.env]]
        name = "TAIGA_BASE_URL"

        [[server.env]]
        name = "TAIGA_PASSWORD"
    "#;

    fn http_catalog(env_var: &str, port_env: &str, headers: &str) -> String {
        format!(
            r#"
            [[server]]
            id = "taiga"
            mode = "http"
            runtime = "bun"
            package = "taiga@1.0.2"

            [[server.env]]
            name = "{env_var}"

            [server.http]
            port_env = "{port_env}"
            headers = {headers}
            "#
        )
    }

    fn with_extra_env(fields: &str) -> String {
        format!("{STDIO}\n[[server.env]]\nname = \"EXTRA\"\n{fields}\n")
    }

    fn with_env_var(name: &str) -> String {
        STDIO.replace("TAIGA_PASSWORD", name)
    }

    #[test]
    fn example_catalog_is_valid() {
        let catalog = Catalog::parse(include_str!("../../../catalog.example.toml")).unwrap();
        assert_eq!(catalog.get("taiga-http").unwrap().mode, ServerMode::Http);
        assert_eq!(catalog.get("firefly").unwrap().mode, ServerMode::Stdio);
    }

    #[test]
    fn header_vars_are_separated_from_process_env() {
        let catalog = Catalog::parse(include_str!("../../../catalog.example.toml")).unwrap();
        let http = catalog.get("taiga-http").unwrap().http.as_ref().unwrap();
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
    fn python_packages_pin_exact_versions_and_name_a_bin() {
        for spec in [
            "a@1.2.3",
            "mcp_server-Time@2024.1",
            "a.b@1.0rc1",
            "a@1.0.post1",
        ] {
            assert!(split_python_package_spec(spec).is_ok(), "{spec}");
        }
        for spec in [
            "a",
            "a@latest",
            "a@>=1.0",
            "a@~=1.0",
            "a@==1.0",
            "a@1.*",
            "a@1.0,<2",
            "-e@1.0",
            "--index-url@1.0",
            "../a@1.0",
            "a@https://example.com/a.whl",
        ] {
            assert!(split_python_package_spec(spec).is_err(), "{spec}");
        }

        let server = |runtime: &str, bin: &str| {
            format!(
                r#"
                [[server]]
                id = "demo"
                mode = "stdio"
                runtime = "{runtime}"
                package = "demo@1.0"
                {bin}
                "#
            )
        };
        for runtime in ["uv", "pip"] {
            assert!(Catalog::parse(&server(runtime, r#"bin = "demo""#)).is_ok());
            assert!(Catalog::parse(&server(runtime, "")).is_err());
            assert!(Catalog::parse(&server(runtime, r#"bin = "../python""#)).is_err());
        }
    }

    #[test]
    fn binary_packages_are_absolute_paths_or_bare_names() {
        let server = |package: &str, bin: &str| {
            format!(
                r#"
                [[server]]
                id = "demo"
                mode = "stdio"
                runtime = "binary"
                package = "{package}"
                {bin}
                "#
            )
        };
        for package in ["/opt/mcp/server", "/usr/bin/my.server-1", "my-server"] {
            assert!(Catalog::parse(&server(package, "")).is_ok(), "{package}");
        }
        for package in [
            "",
            "./server",
            "bin/server",
            "../server",
            "/opt/../bin/sh",
            ".hidden",
        ] {
            assert!(Catalog::parse(&server(package, "")).is_err(), "{package}");
        }
        assert!(Catalog::parse(&server("/opt/mcp/server", r#"bin = "other""#)).is_err());
    }

    #[test]
    fn http_table_must_match_the_mode() {
        let stdio_with_http = format!("{STDIO}\n[server.http]\nport_env = \"PORT\"\n");
        assert!(Catalog::parse(&stdio_with_http).is_err());
        assert!(Catalog::parse(&STDIO.replace("\"stdio\"", "\"http\"")).is_err());
    }

    #[test]
    fn http_rules_are_enforced() {
        let valid = http_catalog("TOKEN", "PORT", r#"{ Authorization = "Bearer {TOKEN}" }"#);
        assert!(Catalog::parse(&valid).is_ok());

        let port_in_env = http_catalog("PORT", "PORT", "{}");
        assert!(Catalog::parse(&port_in_env).is_err());

        let unknown_var = http_catalog("TOKEN", "PORT", r#"{ X-A = "{OTHER}" }"#);
        assert!(Catalog::parse(&unknown_var).is_err());

        let unclosed = http_catalog("TOKEN", "PORT", r#"{ X-A = "{TOKEN" }"#);
        assert!(Catalog::parse(&unclosed).is_err());

        for header in [
            "Host",
            "Content-Length",
            "Mcp-Session-Id",
            "MCP-Protocol-Version",
        ] {
            let forbidden = http_catalog("TOKEN", "PORT", &format!(r#"{{ {header} = "x" }}"#));
            assert!(Catalog::parse(&forbidden).is_err(), "{header}");
        }

        let bad_name = http_catalog("TOKEN", "PORT", r#"{ "Bad Header" = "x" }"#);
        assert!(Catalog::parse(&bad_name).is_err());

        let denied_port = http_catalog("TOKEN", "NODE_PORT", "{}");
        assert!(Catalog::parse(&denied_port).is_err());
    }

    #[test]
    fn env_specs_default_to_secret_optional_strings() {
        let catalog = Catalog::parse(STDIO).unwrap();
        let server = catalog.get("taiga-password").unwrap();
        let spec = server.env_spec("TAIGA_PASSWORD").unwrap();
        assert!(spec.secret);
        assert!(!spec.required);
        assert_eq!(spec.kind, EnvType::String);
        assert!(server.enabled);
        assert!(server.bin_args.is_empty());
    }

    #[test]
    fn enum_values_must_match_the_type() {
        assert!(Catalog::parse(&with_extra_env("type = \"enum\"\nenum_values = [\"a\"]")).is_ok());
        assert!(Catalog::parse(&with_extra_env("type = \"enum\"")).is_err());
        assert!(Catalog::parse(&with_extra_env("enum_values = [\"a\"]")).is_err());
        assert!(Catalog::parse(&with_extra_env("type = \"nonsense\"")).is_err());
    }

    #[test]
    fn conflicts_must_be_known_and_symmetric() {
        let declare = |first: &str, second: &str| {
            format!(
                r#"
                [[server]]
                id = "demo"
                mode = "stdio"
                runtime = "bun"
                package = "demo@1.0.0"

                [[server.env]]
                name = "A"
                conflicts = {first}

                [[server.env]]
                name = "B"
                conflicts = {second}
                "#
            )
        };
        assert!(Catalog::parse(&declare(r#"["B"]"#, r#"["A"]"#)).is_ok());
        assert!(Catalog::parse(&declare(r#"["B"]"#, "[]")).is_err());
        assert!(Catalog::parse(&declare(r#"["A"]"#, "[]")).is_err());
        assert!(Catalog::parse(&declare(r#"["C"]"#, "[]")).is_err());
    }

    #[test]
    fn values_are_checked_against_their_type() {
        let spec = |kind, enum_values: &[&str]| EnvSpec {
            name: "VAR".to_owned(),
            description: String::new(),
            kind,
            enum_values: enum_values
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            secret: true,
            required: false,
            conflicts: Vec::new(),
        };
        let checks = [
            (spec(EnvType::String, &[]), "anything", true),
            (spec(EnvType::String, &[]), "", true),
            (spec(EnvType::Number, &[]), "12", true),
            (spec(EnvType::Number, &[]), "-1.5", true),
            (spec(EnvType::Number, &[]), "abc", false),
            (spec(EnvType::Number, &[]), "NaN", false),
            (spec(EnvType::Number, &[]), "inf", false),
            (spec(EnvType::Boolean, &[]), "true", true),
            (spec(EnvType::Boolean, &[]), "false", true),
            (spec(EnvType::Boolean, &[]), "TRUE", false),
            (spec(EnvType::Boolean, &[]), "1", false),
            (spec(EnvType::Enum, &["a", "b"]), "b", true),
            (spec(EnvType::Enum, &["a", "b"]), "c", false),
        ];
        for (spec, value, expected) in checks {
            assert_eq!(
                spec.validate_value(value).is_ok(),
                expected,
                "{:?} {value}",
                spec.kind
            );
        }

        let mut required = spec(EnvType::String, &[]);
        required.required = true;
        assert!(required.validate_value("").is_err());
        let error = spec(EnvType::Number, &[])
            .validate_value("hunter2")
            .unwrap_err();
        assert!(!error.contains("hunter2"));
    }

    #[test]
    fn a_conflicting_var_satisfies_a_required_one() {
        let catalog = Catalog::parse(include_str!("../../../catalog.example.toml")).unwrap();
        let server = catalog.get("taiga-stdio").unwrap();
        let missing = |set: &[&str]| -> Vec<String> {
            let set_vars: HashSet<&str> = set.iter().copied().collect();
            server
                .missing_required(&set_vars)
                .into_iter()
                .map(str::to_owned)
                .collect()
        };

        assert_eq!(
            missing(&[]),
            [
                "TAIGA_BASE_URL",
                "TAIGA_USERNAME",
                "TAIGA_PASSWORD",
                "TAIGA_TOKEN"
            ]
        );
        assert_eq!(missing(&["TAIGA_BASE_URL", "TAIGA_TOKEN"]), [""; 0]);
        assert_eq!(
            missing(&["TAIGA_BASE_URL", "TAIGA_USERNAME", "TAIGA_PASSWORD"]),
            [""; 0]
        );
        assert_eq!(
            missing(&["TAIGA_BASE_URL", "TAIGA_USERNAME"]),
            ["TAIGA_PASSWORD"]
        );
    }

    #[test]
    fn old_env_vars_key_is_rejected() {
        let old = r#"
            [[server]]
            id = "demo"
            mode = "stdio"
            runtime = "bun"
            package = "demo@1.0.0"
            env_vars = ["A"]
        "#;
        assert!(Catalog::parse(old).is_err());
    }

    #[test]
    fn ids_and_duplicates_are_checked() {
        assert!(Catalog::parse(&STDIO.replace("taiga-password", "Bad_Id")).is_err());
        assert!(Catalog::parse(&format!("{STDIO}\n{STDIO}")).is_err());
        assert!(Catalog::parse(&STDIO.replace("bin =", "command =")).is_err());
    }
}
