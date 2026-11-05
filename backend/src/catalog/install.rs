//! Installs catalog packages ahead of time and finds the script each one runs.
//! Nothing here runs during a user request, so `npx`/`bunx` never happens at spawn.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use tokio::process::Command;

use super::{Catalog, ServerTemplate, split_package_spec};

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("server {server}: {reason}")]
    Failed { server: String, reason: String },
}

fn failed(server: &ServerTemplate, reason: impl Into<String>) -> InstallError {
    InstallError::Failed {
        server: server.id.clone(),
        reason: reason.into(),
    }
}

/// Installs every enabled server's package and returns each one's resolved bin script.
pub async fn install_all(
    catalog: &Catalog,
    packages_dir: &Path,
    bun: &Path,
) -> Result<HashMap<String, PathBuf>, InstallError> {
    let mut scripts = HashMap::new();
    for server in catalog.servers().filter(|server| server.enabled) {
        install_server(server, packages_dir, bun).await?;
        scripts.insert(server.id.clone(), resolve_bin_script(server, packages_dir)?);
    }
    Ok(scripts)
}

async fn install_server(
    server: &ServerTemplate,
    packages_dir: &Path,
    bun: &Path,
) -> Result<(), InstallError> {
    let (name, version) = split_package_spec(&server.package).map_err(|e| failed(server, e))?;
    let directory = packages_dir.join(&server.id);
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(|e| failed(server, format!("creating {}: {e}", directory.display())))?;

    let manifest = json!({ "private": true, "dependencies": { name: version } });
    tokio::fs::write(directory.join("package.json"), manifest.to_string())
        .await
        .map_err(|e| failed(server, format!("writing package.json: {e}")))?;

    tracing::info!(server = %server.id, package = %server.package, "installing package");
    let output = Command::new(bun)
        .args(["install", "--ignore-scripts"])
        .current_dir(&directory)
        .env_clear()
        .env("PATH", crate::process::GATEWAY_PATH)
        .env("HOME", &directory)
        .env("BUN_INSTALL_CACHE_DIR", packages_dir.join(".bun-cache"))
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| failed(server, format!("running {}: {e}", bun.display())))?;
    if !output.status.success() {
        return Err(failed(
            server,
            format!(
                "bun install failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(())
}

/// Finds the script from the installed package's `bin` field.
pub fn resolve_bin_script(
    server: &ServerTemplate,
    packages_dir: &Path,
) -> Result<PathBuf, InstallError> {
    let (name, _) = split_package_spec(&server.package).map_err(|e| failed(server, e))?;
    let package_dir = packages_dir
        .join(&server.id)
        .join("node_modules")
        .join(name);
    let manifest_path = package_dir.join("package.json");
    let manifest: Value = std::fs::read_to_string(&manifest_path)
        .map_err(|e| e.to_string())
        .and_then(|text| serde_json::from_str(&text).map_err(|e| e.to_string()))
        .map_err(|e| failed(server, format!("reading {}: {e}", manifest_path.display())))?;

    let unscoped_name = name.rsplit('/').next().unwrap_or(name);
    let relative = select_bin(&manifest["bin"], server.bin.as_deref(), unscoped_name)
        .map_err(|e| failed(server, e))?;

    let script = package_dir
        .join(relative)
        .canonicalize()
        .map_err(|e| failed(server, format!("bin script {relative}: {e}")))?;
    let root = package_dir
        .canonicalize()
        .map_err(|e| failed(server, format!("package directory: {e}")))?;
    if !script.starts_with(&root) {
        return Err(failed(
            server,
            format!("bin script {relative} is outside the package"),
        ));
    }
    Ok(script)
}

fn select_bin<'a>(
    bin: &'a Value,
    wanted: Option<&str>,
    unscoped_name: &str,
) -> Result<&'a str, String> {
    match bin {
        // A string `bin` is one entry named after the package.
        Value::String(path) => match wanted {
            Some(name) if name != unscoped_name => Err(format!("package has no bin named {name}")),
            _ => Ok(path),
        },
        Value::Object(entries) => {
            let selected = match wanted {
                Some(name) => entries
                    .get(name)
                    .ok_or_else(|| format!("package has no bin named {name}"))?,
                None if entries.len() == 1 => entries.values().next().expect("length is 1"),
                None => return Err("package has several bins; set `bin` in the catalog".to_owned()),
            };
            selected
                .as_str()
                .ok_or_else(|| "package bin entry is not a string".to_owned())
        }
        _ => Err("package declares no bin".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn catalog(package: &str, bin: Option<&str>) -> Catalog {
        let bin_line = bin.map_or(String::new(), |name| format!("bin = \"{name}\""));
        Catalog::parse(&format!(
            r#"
            [[server]]
            id = "demo"
            mode = "stdio"
            runtime = "bun"
            package = "{package}"
            {bin_line}
            "#
        ))
        .unwrap()
    }

    fn write_package(root: &Path, name: &str, manifest: Value) -> PathBuf {
        let directory = root.join("demo/node_modules").join(name);
        std::fs::create_dir_all(directory.join("dist")).unwrap();
        std::fs::write(directory.join("package.json"), manifest.to_string()).unwrap();
        std::fs::write(directory.join("dist/cli.js"), "").unwrap();
        directory.join("dist/cli.js").canonicalize().unwrap()
    }

    #[test]
    fn resolves_string_and_map_bins() {
        let root = tempfile::tempdir().unwrap();
        let script = write_package(root.path(), "@s/demo", json!({ "bin": "dist/cli.js" }));
        let catalog = catalog("@s/demo@1.0.0", None);
        let server = catalog.get("demo").unwrap();
        assert_eq!(resolve_bin_script(server, root.path()).unwrap(), script);

        write_package(
            root.path(),
            "@s/demo",
            json!({ "bin": { "one": "./dist/cli.js", "two": "./dist/other.js" } }),
        );
        assert!(resolve_bin_script(server, root.path()).is_err());

        let named = Catalog::parse(
            r#"
            [[server]]
            id = "demo"
            mode = "stdio"
            runtime = "bun"
            package = "@s/demo@1.0.0"
            bin = "one"
            "#,
        )
        .unwrap();
        assert_eq!(
            resolve_bin_script(named.get("demo").unwrap(), root.path()).unwrap(),
            script
        );
    }

    #[test]
    fn rejects_a_missing_bin_and_a_path_outside_the_package() {
        let root = tempfile::tempdir().unwrap();
        let named = catalog("demo@1.0.0", Some("nope"));
        write_package(root.path(), "demo", json!({ "bin": "dist/cli.js" }));
        assert!(resolve_bin_script(named.get("demo").unwrap(), root.path()).is_err());

        std::fs::write(root.path().join("outside.js"), "").unwrap();
        write_package(root.path(), "demo", json!({ "bin": "../../../outside.js" }));
        let catalog = catalog("demo@1.0.0", None);
        let error = resolve_bin_script(catalog.get("demo").unwrap(), root.path()).unwrap_err();
        assert!(error.to_string().contains("outside the package"));
    }

    #[tokio::test]
    async fn installs_with_a_generated_manifest_and_no_scripts() {
        let root = tempfile::tempdir().unwrap();
        // A fake bun: records its arguments, then "installs" the package.
        let fake_bun = root.path().join("fake-bun.sh");
        std::fs::write(
            &fake_bun,
            r#"#!/bin/sh
echo "$@" > args.txt
env > env.txt
mkdir -p node_modules/demo
echo '{"bin":"cli.js"}' > node_modules/demo/package.json
touch node_modules/demo/cli.js
"#,
        )
        .unwrap();
        std::fs::set_permissions(&fake_bun, std::fs::Permissions::from_mode(0o755)).unwrap();

        let packages = root.path().join("packages");
        let scripts = install_all(&catalog("demo@1.2.3", None), &packages, &fake_bun)
            .await
            .unwrap();

        let directory = packages.join("demo");
        assert!(scripts["demo"].ends_with("node_modules/demo/cli.js"));
        let manifest: Value =
            serde_json::from_str(&std::fs::read_to_string(directory.join("package.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["dependencies"]["demo"], "1.2.3");
        let args = std::fs::read_to_string(directory.join("args.txt")).unwrap();
        assert_eq!(args.trim(), "install --ignore-scripts");
        let env = std::fs::read_to_string(directory.join("env.txt")).unwrap();
        assert!(!env.contains("DATABASE_URL"));
    }

    #[tokio::test]
    async fn disabled_servers_are_not_installed() {
        let root = tempfile::tempdir().unwrap();
        let catalog = Catalog::parse(
            r#"
            [[server]]
            id = "demo"
            enabled = false
            mode = "stdio"
            runtime = "bun"
            package = "demo@1.0.0"
            "#,
        )
        .unwrap();
        let scripts = install_all(&catalog, root.path(), Path::new("/nonexistent/bun"))
            .await
            .unwrap();
        assert!(scripts.is_empty());
    }

    #[tokio::test]
    async fn a_failed_install_names_the_server() {
        let root = tempfile::tempdir().unwrap();
        let fake_bun = root.path().join("fake-bun.sh");
        std::fs::write(&fake_bun, "#!/bin/sh\necho boom >&2\nexit 1\n").unwrap();
        std::fs::set_permissions(&fake_bun, std::fs::Permissions::from_mode(0o755)).unwrap();

        let error = install_all(&catalog("demo@1.2.3", None), root.path(), &fake_bun)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("demo"));
        assert!(error.to_string().contains("boom"));
    }
}
