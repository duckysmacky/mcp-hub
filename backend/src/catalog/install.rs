//! Installs catalog packages ahead of time and finds the script each one runs.
//! Nothing here runs during a user request, so `npx`/`bunx` never happens at spawn.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use tokio::process::Command;

use super::{Catalog, RuntimeKind, ServerTemplate};
use crate::config::RuntimeSettings;

/// A Python server's virtual environment, relative to its package directory.
const UV_VENV: &str = ".venv";
/// Where pip puts a Python server's packages, relative to its package directory.
const PIP_TARGET: &str = "site";

/// What a server's processes run.
#[derive(Clone, Debug)]
pub struct ResolvedServer {
    pub script: PathBuf,
    /// Set for pip servers, whose packages live outside any environment's default path.
    pub python_path: Option<PathBuf>,
}

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

/// Installs every enabled server's package and returns what each one runs.
pub async fn install_all(
    catalog: &Catalog,
    packages_dir: &Path,
    runtimes: &RuntimeSettings,
) -> Result<HashMap<String, ResolvedServer>, InstallError> {
    let mut resolved = HashMap::new();
    for server in catalog.servers().filter(|server| server.enabled) {
        install_server(server, packages_dir, runtimes).await?;
        resolved.insert(server.id.clone(), resolve_server(server, packages_dir)?);
    }
    Ok(resolved)
}

async fn install_server(
    server: &ServerTemplate,
    packages_dir: &Path,
    runtimes: &RuntimeSettings,
) -> Result<(), InstallError> {
    let (name, version) = server.package_parts().map_err(|e| failed(server, e))?;
    let directory = packages_dir.join(&server.id);
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(|e| failed(server, format!("creating {}: {e}", directory.display())))?;
    tracing::info!(server = %server.id, package = %server.package, "installing package");

    let requirement = format!("{name}=={version}");
    // `--only-binary` is Python's `--ignore-scripts`: no package code runs at install.
    match server.runtime {
        RuntimeKind::Bun | RuntimeKind::Node => {
            let manifest = json!({ "private": true, "dependencies": { name: version } });
            tokio::fs::write(directory.join("package.json"), manifest.to_string())
                .await
                .map_err(|e| failed(server, format!("writing package.json: {e}")))?;
            let mut command = installer(&runtimes.bun, &directory);
            command
                .args(["install", "--ignore-scripts"])
                .env("BUN_INSTALL_CACHE_DIR", packages_dir.join(".bun-cache"));
            run(server, command).await
        }
        RuntimeKind::Uv => {
            let uv = |args: &[&str]| {
                let mut command = installer(&runtimes.uv, &directory);
                command
                    .args(args)
                    .env("UV_CACHE_DIR", packages_dir.join(".uv-cache"))
                    .env("UV_PYTHON_DOWNLOADS", "never");
                command
            };
            let venv_python = format!("{UV_VENV}/bin/python");
            run(server, uv(&["venv", "--allow-existing", UV_VENV])).await?;
            let install = [
                "pip",
                "install",
                "--python",
                &venv_python,
                "--only-binary",
                ":all:",
                &requirement,
            ];
            run(server, uv(&install)).await
        }
        RuntimeKind::Pip => {
            let mut command = installer(&runtimes.pip, &directory);
            command
                .args([
                    "install",
                    "--target",
                    PIP_TARGET,
                    "--upgrade",
                    "--only-binary",
                    ":all:",
                    "--no-input",
                    "--disable-pip-version-check",
                    &requirement,
                ])
                .env("PIP_CACHE_DIR", packages_dir.join(".pip-cache"));
            run(server, command).await
        }
    }
}

/// A package tool's command: nothing from the hub's own environment reaches it.
fn installer(program: &Path, directory: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .current_dir(directory)
        .env_clear()
        .env("PATH", crate::process::GATEWAY_PATH)
        .env("HOME", directory)
        .kill_on_drop(true);
    command
}

async fn run(server: &ServerTemplate, mut command: Command) -> Result<(), InstallError> {
    let program = command.as_std().get_program().to_owned();
    let output = command
        .output()
        .await
        .map_err(|e| failed(server, format!("running {}: {e}", program.display())))?;
    if !output.status.success() {
        return Err(failed(
            server,
            format!(
                "{} failed: {}",
                program.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(())
}

/// Finds what an installed server runs.
pub fn resolve_server(
    server: &ServerTemplate,
    packages_dir: &Path,
) -> Result<ResolvedServer, InstallError> {
    let directory = packages_dir.join(&server.id);
    let python_entry = |root: &Path| {
        // The catalog guarantees Python servers a plain `bin` name.
        let script = root
            .join("bin")
            .join(server.bin.as_deref().unwrap_or_default());
        script
            .canonicalize()
            .map_err(|e| failed(server, format!("bin script {}: {e}", script.display())))
    };
    match server.runtime {
        RuntimeKind::Bun | RuntimeKind::Node => Ok(ResolvedServer {
            script: resolve_bin_script(server, packages_dir)?,
            python_path: None,
        }),
        RuntimeKind::Uv => Ok(ResolvedServer {
            script: python_entry(&directory.join(UV_VENV))?,
            python_path: None,
        }),
        RuntimeKind::Pip => {
            let root = directory.join(PIP_TARGET);
            Ok(ResolvedServer {
                script: python_entry(&root)?,
                python_path: Some(root),
            })
        }
    }
}

/// Finds the script from the installed package's `bin` field.
pub fn resolve_bin_script(
    server: &ServerTemplate,
    packages_dir: &Path,
) -> Result<PathBuf, InstallError> {
    let (name, _) = server.package_parts().map_err(|e| failed(server, e))?;
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

    fn runtimes_with(tool: &Path) -> RuntimeSettings {
        RuntimeSettings {
            bun: tool.to_owned(),
            node: tool.to_owned(),
            uv: tool.to_owned(),
            pip: tool.to_owned(),
        }
    }

    fn write_script(path: &Path, text: &str) {
        std::fs::write(path, text).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

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
        write_script(
            &fake_bun,
            r#"#!/bin/sh
echo "$@" > args.txt
env > env.txt
mkdir -p node_modules/demo
echo '{"bin":"cli.js"}' > node_modules/demo/package.json
touch node_modules/demo/cli.js
"#,
        );

        let packages = root.path().join("packages");
        let resolved = install_all(
            &catalog("demo@1.2.3", None),
            &packages,
            &runtimes_with(&fake_bun),
        )
        .await
        .unwrap();

        let directory = packages.join("demo");
        assert!(
            resolved["demo"]
                .script
                .ends_with("node_modules/demo/cli.js")
        );
        assert!(resolved["demo"].python_path.is_none());
        let manifest: Value =
            serde_json::from_str(&std::fs::read_to_string(directory.join("package.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["dependencies"]["demo"], "1.2.3");
        let args = std::fs::read_to_string(directory.join("args.txt")).unwrap();
        assert_eq!(args.trim(), "install --ignore-scripts");
        let env = std::fs::read_to_string(directory.join("env.txt")).unwrap();
        assert!(!env.contains("DATABASE_URL"));
    }

    fn python_catalog(runtime: &str) -> Catalog {
        Catalog::parse(&format!(
            r#"
            [[server]]
            id = "demo"
            mode = "stdio"
            runtime = "{runtime}"
            package = "demo-pkg@1.2"
            bin = "demo"
            "#
        ))
        .unwrap()
    }

    #[tokio::test]
    async fn uv_installs_binaries_only_into_a_private_venv() {
        let root = tempfile::tempdir().unwrap();
        let fake_uv = root.path().join("fake-uv.sh");
        write_script(
            &fake_uv,
            r#"#!/bin/sh
echo "$@" >> args.txt
env > env.txt
mkdir -p .venv/bin
touch .venv/bin/demo
"#,
        );
        let packages = root.path().join("packages");
        let resolved = install_all(&python_catalog("uv"), &packages, &runtimes_with(&fake_uv))
            .await
            .unwrap();

        assert!(resolved["demo"].script.ends_with(".venv/bin/demo"));
        assert!(resolved["demo"].python_path.is_none());
        let args = std::fs::read_to_string(packages.join("demo/args.txt")).unwrap();
        assert!(args.contains("venv --allow-existing .venv"), "{args}");
        assert!(args.contains("--only-binary :all: demo-pkg==1.2"), "{args}");
        let env = std::fs::read_to_string(packages.join("demo/env.txt")).unwrap();
        assert!(env.contains("UV_PYTHON_DOWNLOADS=never"));
    }

    #[tokio::test]
    async fn pip_installs_binaries_only_into_its_own_directory() {
        let root = tempfile::tempdir().unwrap();
        let fake_pip = root.path().join("fake-pip.sh");
        write_script(
            &fake_pip,
            r#"#!/bin/sh
echo "$@" > args.txt
mkdir -p site/bin
touch site/bin/demo
"#,
        );
        let packages = root.path().join("packages");
        let resolved = install_all(&python_catalog("pip"), &packages, &runtimes_with(&fake_pip))
            .await
            .unwrap();

        assert!(resolved["demo"].script.ends_with("site/bin/demo"));
        assert!(
            resolved["demo"]
                .python_path
                .as_ref()
                .unwrap()
                .ends_with("site")
        );
        let args = std::fs::read_to_string(packages.join("demo/args.txt")).unwrap();
        assert!(args.contains("--target site"), "{args}");
        assert!(args.contains("--only-binary :all:"), "{args}");
        assert!(args.ends_with("demo-pkg==1.2\n"), "{args}");
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
        let resolved = install_all(
            &catalog,
            root.path(),
            &runtimes_with(Path::new("/nonexistent/bun")),
        )
        .await
        .unwrap();
        assert!(resolved.is_empty());
    }

    #[tokio::test]
    async fn a_failed_install_names_the_server() {
        let root = tempfile::tempdir().unwrap();
        let fake_bun = root.path().join("fake-bun.sh");
        write_script(&fake_bun, "#!/bin/sh\necho boom >&2\nexit 1\n");

        let error = install_all(
            &catalog("demo@1.2.3", None),
            root.path(),
            &runtimes_with(&fake_bun),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("demo"));
        assert!(error.to_string().contains("boom"));
    }
}
