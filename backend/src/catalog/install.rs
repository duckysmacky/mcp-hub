//! Installs catalog packages ahead of time and finds the script each one runs.
//! Nothing here runs during a user request, so `npx`/`bunx` never happens at spawn.

use std::{
    collections::HashMap,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::process::Command;

use super::{
    Catalog, RuntimeKind, ServerTemplate,
    lock::{CatalogLock, LockError, LockedServer},
};
use crate::config::{RuntimeSettings, Settings};

/// A Python server's virtual environment, relative to its package directory.
const UV_VENV: &str = ".venv";
/// Where pip puts a Python server's packages, relative to its package directory.
const PIP_TARGET: &str = "site";
const BUN_LOCKFILE: &str = "bun.lock";
const PYTHON_LOCKFILE: &str = "requirements.lock";

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
    #[error(transparent)]
    Lock(#[from] LockError),
}

fn failed(server: &ServerTemplate, reason: impl Into<String>) -> InstallError {
    InstallError::Failed {
        server: server.id.clone(),
        reason: reason.into(),
    }
}

/// The lock to install from. With `catalog.require_lock` it's the committed file, which must
/// match exactly. Without, whatever the file lacks is resolved now and then installed the same
/// frozen way, but nobody has reviewed that tree and it can differ between starts.
pub async fn effective_lock(
    catalog: &Catalog,
    settings: &Settings,
) -> Result<CatalogLock, InstallError> {
    let paths = &settings.paths;
    let path = CatalogLock::path_for(&paths.catalog);
    if settings.catalog.require_lock {
        return Ok(CatalogLock::load_verified(catalog, &path)?);
    }
    let mut lock = CatalogLock::load_lenient(catalog, &path)?;
    lock_missing(
        catalog,
        &mut lock,
        &paths.packages,
        &paths.scratch.join("catalog-lock"),
        &settings.runtimes,
    )
    .await?;
    Ok(lock)
}

/// Installs every enabled server's locked packages and returns what each one runs.
pub async fn install_all(
    catalog: &Catalog,
    lock: &CatalogLock,
    packages_dir: &Path,
    runtimes: &RuntimeSettings,
) -> Result<HashMap<String, ResolvedServer>, InstallError> {
    let mut resolved = HashMap::new();
    for server in catalog.servers().filter(|server| server.enabled) {
        install_server(server, lock, packages_dir, runtimes).await?;
        resolved.insert(server.id.clone(), resolve_server(server, packages_dir)?);
    }
    Ok(resolved)
}

/// Removes a server's installed files and installs it again from the lockfile.
pub async fn reinstall_server(
    server: &ServerTemplate,
    lock: &CatalogLock,
    packages_dir: &Path,
    runtimes: &RuntimeSettings,
) -> Result<(), InstallError> {
    let directory = packages_dir.join(&server.id);
    match tokio::fs::remove_dir_all(&directory).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(failed(
                server,
                format!("removing {}: {error}", directory.display()),
            ));
        }
    }
    install_server(server, lock, packages_dir, runtimes).await?;
    resolve_server(server, packages_dir).map(|_| ())
}

async fn write_file(server: &ServerTemplate, path: &Path, text: &str) -> Result<(), InstallError> {
    tokio::fs::write(path, text)
        .await
        .map_err(|e| failed(server, format!("writing {}: {e}", path.display())))
}

async fn create_directory(server: &ServerTemplate, directory: &Path) -> Result<(), InstallError> {
    tokio::fs::create_dir_all(directory)
        .await
        .map_err(|e| failed(server, format!("creating {}: {e}", directory.display())))
}

fn bun_manifest(server: &ServerTemplate) -> Result<String, InstallError> {
    let (name, version) = server.package_parts().map_err(|e| failed(server, e))?;
    Ok(json!({ "private": true, "dependencies": { name: version } }).to_string())
}

fn bun_command(runtimes: &RuntimeSettings, packages_dir: &Path, directory: &Path) -> Command {
    let mut command = installer(&runtimes.bun, directory);
    command.env("BUN_INSTALL_CACHE_DIR", packages_dir.join(".bun-cache"));
    command
}

fn uv_command(runtimes: &RuntimeSettings, packages_dir: &Path, directory: &Path) -> Command {
    let mut command = installer(&runtimes.uv, directory);
    command
        .env("UV_CACHE_DIR", packages_dir.join(".uv-cache"))
        .env("UV_PYTHON_DOWNLOADS", "never");
    command
}

fn pip_command(runtimes: &RuntimeSettings, packages_dir: &Path, directory: &Path) -> Command {
    let mut command = installer(&runtimes.pip, directory);
    command
        .env("PIP_CACHE_DIR", packages_dir.join(".pip-cache"))
        .args(["install", "--no-input", "--disable-pip-version-check"]);
    command
}

/// Installs exactly what the lockfile lists. `--only-binary` is Python's
/// `--ignore-scripts`: no package code runs at install.
async fn install_server(
    server: &ServerTemplate,
    lock: &CatalogLock,
    packages_dir: &Path,
    runtimes: &RuntimeSettings,
) -> Result<(), InstallError> {
    let locked = lock
        .get(&server.id)
        .ok_or_else(|| failed(server, "not in the lockfile"))?;
    let directory = packages_dir.join(&server.id);
    // A binary stays where the operator put it.
    if server.runtime != RuntimeKind::Binary {
        create_directory(server, &directory).await?;
    }
    tracing::info!(server = %server.id, package = %server.package, "installing package");

    match server.runtime {
        RuntimeKind::Bun | RuntimeKind::Node => {
            write_file(
                server,
                &directory.join("package.json"),
                &bun_manifest(server)?,
            )
            .await?;
            write_file(server, &directory.join(BUN_LOCKFILE), &locked.lock).await?;
            let mut command = bun_command(runtimes, packages_dir, &directory);
            command.args(["install", "--frozen-lockfile", "--ignore-scripts"]);
            run(server, command).await?;
        }
        RuntimeKind::Uv => {
            write_file(server, &directory.join(PYTHON_LOCKFILE), &locked.lock).await?;
            let mut venv = uv_command(runtimes, packages_dir, &directory);
            venv.args(["venv", "--allow-existing", UV_VENV]);
            run(server, venv).await?;
            let mut install = uv_command(runtimes, packages_dir, &directory);
            install.args([
                "pip",
                "install",
                "--python",
                &format!("{UV_VENV}/bin/python"),
                "--require-hashes",
                "--only-binary",
                ":all:",
                "-r",
                PYTHON_LOCKFILE,
            ]);
            run(server, install).await?;
        }
        RuntimeKind::Pip => {
            write_file(server, &directory.join(PYTHON_LOCKFILE), &locked.lock).await?;
            let mut command = pip_command(runtimes, packages_dir, &directory);
            command.args([
                "--target",
                PIP_TARGET,
                "--upgrade",
                "--require-hashes",
                "--only-binary",
                ":all:",
                "-r",
                PYTHON_LOCKFILE,
            ]);
            run(server, command).await?;
        }
        RuntimeKind::Binary => {
            if binary_hash(server).await? != locked.lock {
                return Err(failed(
                    server,
                    "the binary changed since it was locked; run `mcp-hub catalog lock`",
                ));
            }
        }
    }
    Ok(())
}

/// Resolves every enabled server's dependencies from the registries, without installing
/// them or running any package code. `work_dir` is removed afterwards.
pub async fn generate_lock(
    catalog: &Catalog,
    packages_dir: &Path,
    work_dir: &Path,
    runtimes: &RuntimeSettings,
) -> Result<CatalogLock, InstallError> {
    let mut lock = CatalogLock::default();
    lock_missing(catalog, &mut lock, packages_dir, work_dir, runtimes).await?;
    Ok(lock)
}

/// Resolves the enabled servers that `lock` doesn't cover yet.
async fn lock_missing(
    catalog: &Catalog,
    lock: &mut CatalogLock,
    packages_dir: &Path,
    work_dir: &Path,
    runtimes: &RuntimeSettings,
) -> Result<(), InstallError> {
    for server in catalog.servers().filter(|server| server.enabled) {
        if lock.get(&server.id).is_some() {
            continue;
        }
        tracing::info!(server = %server.id, package = %server.package, "locking package");
        let directory = work_dir.join(&server.id);
        create_directory(server, &directory).await?;
        let text = lock_server(server, packages_dir, &directory, runtimes).await;
        // A leftover directory is only clutter, so a failure to remove it isn't one.
        let _ = tokio::fs::remove_dir_all(&directory).await;
        lock.insert(
            server.id.clone(),
            LockedServer {
                package: server.package.clone(),
                runtime: server.runtime,
                lock: text?,
            },
        );
    }
    Ok(())
}

async fn lock_server(
    server: &ServerTemplate,
    packages_dir: &Path,
    directory: &Path,
    runtimes: &RuntimeSettings,
) -> Result<String, InstallError> {
    let python_requirement = || {
        let (name, version) = server.package_parts().map_err(|e| failed(server, e))?;
        Ok::<_, InstallError>(format!("{name}=={version}"))
    };
    match server.runtime {
        RuntimeKind::Bun | RuntimeKind::Node => {
            write_file(
                server,
                &directory.join("package.json"),
                &bun_manifest(server)?,
            )
            .await?;
            let mut command = bun_command(runtimes, packages_dir, directory);
            command.args(["install", "--lockfile-only", "--ignore-scripts"]);
            run(server, command).await?;
            let path = directory.join(BUN_LOCKFILE);
            tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| failed(server, format!("reading {}: {e}", path.display())))
        }
        RuntimeKind::Uv => {
            write_file(
                server,
                &directory.join("requirements.in"),
                &format!("{}\n", python_requirement()?),
            )
            .await?;
            let mut command = uv_command(runtimes, packages_dir, directory);
            command.args([
                "pip",
                "compile",
                "requirements.in",
                "--generate-hashes",
                "--only-binary",
                ":all:",
                "--no-header",
                "--no-annotate",
                "--quiet",
            ]);
            utf8_output(server, run(server, command).await?)
        }
        RuntimeKind::Pip => {
            let mut command = pip_command(runtimes, packages_dir, directory);
            command.args([
                "--dry-run",
                "--ignore-installed",
                "--target",
                PIP_TARGET,
                "--only-binary",
                ":all:",
                "--quiet",
                "--report",
                "-",
                &python_requirement()?,
            ]);
            let report = run(server, command).await?;
            pip_report_to_requirements(&report).map_err(|e| failed(server, e))
        }
        RuntimeKind::Binary => binary_hash(server).await,
    }
}

/// The SHA-256 of a binary server's file, in hex: all a binary's lock needs to hold.
async fn binary_hash(server: &ServerTemplate) -> Result<String, InstallError> {
    let path = binary_path(server)?;
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| failed(server, format!("reading {}: {e}", path.display())))?;
    Ok(Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Finds a binary server's file: an absolute `package` as is, a bare name on the hub's own
/// `PATH`, then the children's. Children run it by this path, so their `PATH` stays the hub's.
fn binary_path(server: &ServerTemplate) -> Result<PathBuf, InstallError> {
    let package = Path::new(&server.package);
    let path = if package.is_absolute() {
        package.to_owned()
    } else {
        let hub_path = std::env::var_os("PATH").unwrap_or_default();
        let directories = std::env::split_paths(&hub_path)
            .chain(std::env::split_paths(crate::process::GATEWAY_PATH));
        find_executable(&server.package, directories)
            .ok_or_else(|| failed(server, format!("{} is not on PATH", server.package)))?
    };
    let path = path
        .canonicalize()
        .map_err(|e| failed(server, format!("binary {}: {e}", path.display())))?;
    if !is_executable(&path) {
        return Err(failed(
            server,
            format!("{} is not an executable file", path.display()),
        ));
    }
    Ok(path)
}

fn find_executable(name: &str, directories: impl Iterator<Item = PathBuf>) -> Option<PathBuf> {
    directories
        .map(|directory| directory.join(name))
        .find(|path| is_executable(path))
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn utf8_output(server: &ServerTemplate, output: Vec<u8>) -> Result<String, InstallError> {
    String::from_utf8(output).map_err(|_| failed(server, "the tool's output isn't UTF-8"))
}

/// Turns a `pip install --report` into requirements with hashes, for `--require-hashes`.
/// pip reports only the wheels for this platform, so lock on the deployment platform.
fn pip_report_to_requirements(report: &[u8]) -> Result<String, String> {
    let report: Value =
        serde_json::from_slice(report).map_err(|e| format!("reading pip's report: {e}"))?;
    let packages = report["install"]
        .as_array()
        .ok_or("pip's report lists no packages")?;
    let mut lines = Vec::with_capacity(packages.len());
    for package in packages {
        let name = package["metadata"]["name"].as_str();
        let version = package["metadata"]["version"].as_str();
        let hash = package["download_info"]["archive_info"]["hashes"]["sha256"].as_str();
        let (Some(name), Some(version), Some(hash)) = (name, version, hash) else {
            return Err("pip's report has a package without a name, version or hash".to_owned());
        };
        lines.push(format!("{name}=={version} --hash=sha256:{hash}\n"));
    }
    lines.sort();
    Ok(lines.concat())
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

/// Runs a package tool and returns its stdout.
async fn run(server: &ServerTemplate, mut command: Command) -> Result<Vec<u8>, InstallError> {
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
    Ok(output.stdout)
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
        RuntimeKind::Binary => Ok(ResolvedServer {
            script: binary_path(server)?,
            python_path: None,
        }),
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

    fn lock_of(catalog: &Catalog, text: &str) -> CatalogLock {
        let mut lock = CatalogLock::default();
        for server in catalog.servers() {
            lock.insert(
                server.id.clone(),
                LockedServer {
                    package: server.package.clone(),
                    runtime: server.runtime,
                    lock: text.to_owned(),
                },
            );
        }
        lock
    }

    #[tokio::test]
    async fn installs_frozen_from_the_lock_with_no_scripts() {
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
        let catalog = catalog("demo@1.2.3", None);
        let lock = lock_of(&catalog, "locked tree");
        let resolved = install_all(&catalog, &lock, &packages, &runtimes_with(&fake_bun))
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
        assert_eq!(
            std::fs::read_to_string(directory.join("bun.lock")).unwrap(),
            "locked tree"
        );
        let args = std::fs::read_to_string(directory.join("args.txt")).unwrap();
        assert_eq!(args.trim(), "install --frozen-lockfile --ignore-scripts");
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
    async fn uv_installs_hashed_binaries_into_a_private_venv() {
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
        let catalog = python_catalog("uv");
        let lock = lock_of(&catalog, "demo-pkg==1.2 --hash=sha256:abc\n");
        let resolved = install_all(&catalog, &lock, &packages, &runtimes_with(&fake_uv))
            .await
            .unwrap();

        assert!(resolved["demo"].script.ends_with(".venv/bin/demo"));
        assert!(resolved["demo"].python_path.is_none());
        let directory = packages.join("demo");
        let args = std::fs::read_to_string(directory.join("args.txt")).unwrap();
        assert!(args.contains("venv --allow-existing .venv"), "{args}");
        assert!(
            args.contains("--require-hashes --only-binary :all: -r requirements.lock"),
            "{args}"
        );
        let env = std::fs::read_to_string(directory.join("env.txt")).unwrap();
        assert!(env.contains("UV_PYTHON_DOWNLOADS=never"));
        assert!(
            std::fs::read_to_string(directory.join("requirements.lock"))
                .unwrap()
                .contains("--hash=sha256:abc")
        );
    }

    #[tokio::test]
    async fn pip_installs_hashed_binaries_into_its_own_directory() {
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
        let catalog = python_catalog("pip");
        let lock = lock_of(&catalog, "demo-pkg==1.2 --hash=sha256:abc\n");
        let resolved = install_all(&catalog, &lock, &packages, &runtimes_with(&fake_pip))
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
        assert!(
            args.contains("--require-hashes --only-binary :all: -r requirements.lock"),
            "{args}"
        );
    }

    #[tokio::test]
    async fn an_unlocked_server_is_not_installed() {
        let root = tempfile::tempdir().unwrap();
        let result = install_all(
            &catalog("demo@1.2.3", None),
            &CatalogLock::default(),
            root.path(),
            &runtimes_with(Path::new("/nonexistent/bun")),
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("lockfile"));
    }

    #[tokio::test]
    async fn disabled_servers_are_not_installed_or_locked() {
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
        let runtimes = runtimes_with(Path::new("/nonexistent/bun"));
        let resolved = install_all(&catalog, &CatalogLock::default(), root.path(), &runtimes)
            .await
            .unwrap();
        assert!(resolved.is_empty());
        let lock = generate_lock(&catalog, root.path(), root.path(), &runtimes)
            .await
            .unwrap();
        assert_eq!(lock, CatalogLock::default());
    }

    #[tokio::test]
    async fn a_failed_install_names_the_server() {
        let root = tempfile::tempdir().unwrap();
        let fake_bun = root.path().join("fake-bun.sh");
        write_script(&fake_bun, "#!/bin/sh\necho boom >&2\nexit 1\n");

        let catalog = catalog("demo@1.2.3", None);
        let error = install_all(
            &catalog,
            &lock_of(&catalog, "tree"),
            root.path(),
            &runtimes_with(&fake_bun),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("demo"));
        assert!(error.to_string().contains("boom"));
    }

    #[tokio::test]
    async fn reinstalling_starts_from_an_empty_directory() {
        let root = tempfile::tempdir().unwrap();
        let fake_bun = root.path().join("fake-bun.sh");
        write_script(
            &fake_bun,
            r#"#!/bin/sh
mkdir -p node_modules/demo
echo '{"bin":"cli.js"}' > node_modules/demo/package.json
touch node_modules/demo/cli.js
"#,
        );
        let packages = root.path().join("packages");
        let leftover = packages.join("demo/left-behind");
        std::fs::create_dir_all(&leftover).unwrap();

        let catalog = catalog("demo@1.2.3", None);
        reinstall_server(
            catalog.get("demo").unwrap(),
            &lock_of(&catalog, "tree"),
            &packages,
            &runtimes_with(&fake_bun),
        )
        .await
        .unwrap();
        assert!(!leftover.exists());
        assert!(packages.join("demo/node_modules/demo/cli.js").exists());
    }

    #[tokio::test]
    async fn bun_lock_comes_from_a_lockfile_only_run() {
        let root = tempfile::tempdir().unwrap();
        let fake_bun = root.path().join("fake-bun.sh");
        write_script(
            &fake_bun,
            r#"#!/bin/sh
[ "$*" = "install --lockfile-only --ignore-scripts" ] || exit 1
[ -f package.json ] || exit 1
echo resolved-tree > bun.lock
"#,
        );
        let work = root.path().join("work");
        let catalog = catalog("demo@1.2.3", None);
        let lock = generate_lock(&catalog, root.path(), &work, &runtimes_with(&fake_bun))
            .await
            .unwrap();
        let locked = lock.get("demo").unwrap();
        assert_eq!(locked.lock, "resolved-tree\n");
        assert_eq!(locked.package, "demo@1.2.3");
        assert!(!work.join("demo").exists());
    }

    #[tokio::test]
    async fn uv_lock_is_the_compiled_output() {
        let root = tempfile::tempdir().unwrap();
        let fake_uv = root.path().join("fake-uv.sh");
        write_script(
            &fake_uv,
            r#"#!/bin/sh
case "$*" in
  *"pip compile requirements.in --generate-hashes --only-binary :all:"*) ;;
  *) exit 1 ;;
esac
grep -q '^demo-pkg==1.2$' requirements.in || exit 1
echo 'demo-pkg==1.2 --hash=sha256:abc'
"#,
        );
        let lock = generate_lock(
            &python_catalog("uv"),
            root.path(),
            &root.path().join("work"),
            &runtimes_with(&fake_uv),
        )
        .await
        .unwrap();
        assert_eq!(
            lock.get("demo").unwrap().lock,
            "demo-pkg==1.2 --hash=sha256:abc\n"
        );
    }

    fn binary_catalog(package: &Path) -> Catalog {
        Catalog::parse(&format!(
            r#"
            [[server]]
            id = "demo"
            mode = "stdio"
            runtime = "binary"
            package = "{}"
            "#,
            package.display()
        ))
        .unwrap()
    }

    #[tokio::test]
    async fn a_binary_is_locked_by_hash_and_refused_once_changed() {
        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join("server");
        write_script(&binary, "#!/bin/sh\n");
        let packages = root.path().join("packages");
        let catalog = binary_catalog(&binary);
        let runtimes = runtimes_with(Path::new("/nonexistent/tool"));

        let lock = generate_lock(&catalog, &packages, &root.path().join("work"), &runtimes)
            .await
            .unwrap();
        // `printf '#!/bin/sh\n' | sha256sum`
        assert_eq!(
            lock.get("demo").unwrap().lock,
            "a8076d3d28d21e02012b20eaf7dbf75409a6277134439025f282e368e3305abf"
        );

        let resolved = install_all(&catalog, &lock, &packages, &runtimes)
            .await
            .unwrap();
        assert_eq!(resolved["demo"].script, binary.canonicalize().unwrap());
        assert!(resolved["demo"].python_path.is_none());
        assert!(!packages.join("demo").exists());

        write_script(&binary, "#!/bin/sh\necho tampered\n");
        let error = install_all(&catalog, &lock, &packages, &runtimes)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("changed since it was locked"));
    }

    #[test]
    fn a_binary_must_be_an_executable_file() {
        let root = tempfile::tempdir().unwrap();
        let plain = root.path().join("plain");
        std::fs::write(&plain, "").unwrap();
        let cases = [
            plain.as_path(),
            root.path(),
            Path::new("/nonexistent/server"),
        ];
        for path in cases {
            let catalog = binary_catalog(path);
            assert!(
                resolve_server(catalog.get("demo").unwrap(), root.path()).is_err(),
                "{}",
                path.display()
            );
        }
    }

    #[test]
    fn a_bare_binary_name_is_found_in_the_first_directory_holding_an_executable() {
        let root = tempfile::tempdir().unwrap();
        let (first, second) = (root.path().join("first"), root.path().join("second"));
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(first.join("server"), "").unwrap();
        write_script(&second.join("server"), "");
        let directories = || [first.clone(), second.clone()].into_iter();

        assert_eq!(
            find_executable("server", directories()),
            Some(second.join("server"))
        );
        assert_eq!(find_executable("missing", directories()), None);
    }

    fn settings_for(root: &Path, tool: &Path, require_lock: bool) -> Settings {
        let mut settings = Settings::default();
        settings.catalog.require_lock = require_lock;
        settings.paths.catalog = root.join("catalog.toml");
        settings.paths.packages = root.join("packages");
        settings.paths.scratch = root.join("scratch");
        settings.runtimes = runtimes_with(tool);
        settings
    }

    #[tokio::test]
    async fn without_a_required_lock_a_missing_entry_is_resolved_then_installed_frozen() {
        let root = tempfile::tempdir().unwrap();
        let fake_bun = root.path().join("fake-bun.sh");
        write_script(
            &fake_bun,
            r#"#!/bin/sh
echo "$@" >> args.txt
echo resolved-tree > bun.lock
mkdir -p node_modules/demo
echo '{"bin":"cli.js"}' > node_modules/demo/package.json
touch node_modules/demo/cli.js
"#,
        );
        let catalog = catalog("demo@1.2.3", None);
        let settings = settings_for(root.path(), &fake_bun, false);

        let lock = effective_lock(&catalog, &settings).await.unwrap();
        assert_eq!(lock.get("demo").unwrap().lock, "resolved-tree\n");

        install_all(
            &catalog,
            &lock,
            &settings.paths.packages,
            &settings.runtimes,
        )
        .await
        .unwrap();
        let args = std::fs::read_to_string(settings.paths.packages.join("demo/args.txt")).unwrap();
        assert_eq!(args.trim(), "install --frozen-lockfile --ignore-scripts");
    }

    #[tokio::test]
    async fn without_a_required_lock_an_existing_entry_is_kept() {
        let root = tempfile::tempdir().unwrap();
        let settings = settings_for(root.path(), Path::new("/nonexistent/bun"), false);
        let catalog = catalog("demo@1.2.3", None);
        let written = lock_of(&catalog, "committed tree");
        std::fs::write(
            CatalogLock::path_for(&settings.paths.catalog),
            toml::to_string_pretty(&written).unwrap(),
        )
        .unwrap();

        assert_eq!(effective_lock(&catalog, &settings).await.unwrap(), written);
    }

    #[tokio::test]
    async fn a_required_lock_still_refuses_when_missing() {
        let root = tempfile::tempdir().unwrap();
        let settings = settings_for(root.path(), Path::new("/nonexistent/bun"), true);
        let error = effective_lock(&catalog("demo@1.2.3", None), &settings)
            .await
            .unwrap_err();
        assert!(matches!(error, InstallError::Lock(LockError::Missing(_))));
    }

    #[test]
    fn pip_reports_become_hashed_requirements() {
        let report = json!({
            "install": [
                {
                    "metadata": { "name": "zeta", "version": "2.0" },
                    "download_info": { "archive_info": { "hashes": { "sha256": "bbb" } } },
                },
                {
                    "metadata": { "name": "alpha", "version": "1.0" },
                    "download_info": { "archive_info": { "hashes": { "sha256": "aaa" } } },
                },
            ],
        });
        assert_eq!(
            pip_report_to_requirements(report.to_string().as_bytes()).unwrap(),
            "alpha==1.0 --hash=sha256:aaa\nzeta==2.0 --hash=sha256:bbb\n"
        );

        let no_hash = json!({
            "install": [{ "metadata": { "name": "a", "version": "1" }, "download_info": {} }],
        });
        assert!(pip_report_to_requirements(no_hash.to_string().as_bytes()).is_err());
        assert!(pip_report_to_requirements(b"not json").is_err());
    }
}
