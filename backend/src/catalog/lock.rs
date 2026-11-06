//! `catalog.lock`: the committed record of every enabled server's whole dependency tree, so
//! that a fresh packages volume installs exactly what the operator vetted (SECURITY.md T2).

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::{Catalog, RuntimeKind};

const LOCK_VERSION: u32 = 1;
const HOW_TO_FIX: &str = "run `mcp-hub catalog lock` and commit the result";

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("{path} is missing; {HOW_TO_FIX}", path = .0.display())]
    Missing(PathBuf),
    #[error("reading {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("parsing {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("{path} has version {found}, this hub reads version {LOCK_VERSION}; {HOW_TO_FIX}", path = .path.display())]
    WrongVersion { path: PathBuf, found: u32 },
    #[error("server {server} is not in the lockfile; {HOW_TO_FIX}")]
    NotLocked { server: String },
    #[error("server {server} changed since it was locked; {HOW_TO_FIX}")]
    Stale { server: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LockedServer {
    pub package: String,
    pub runtime: RuntimeKind,
    /// The package tool's own lockfile: `bun.lock`, or requirements with hashes for Python.
    pub lock: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CatalogLock {
    version: u32,
    #[serde(default)]
    server: BTreeMap<String, LockedServer>,
}

impl Default for CatalogLock {
    fn default() -> Self {
        Self {
            version: LOCK_VERSION,
            server: BTreeMap::new(),
        }
    }
}

impl CatalogLock {
    /// A catalog's lockfile sits next to it.
    pub fn path_for(catalog_path: &Path) -> PathBuf {
        catalog_path.with_extension("lock")
    }

    /// Loads the lockfile, refusing unless it covers every enabled server exactly.
    pub fn load_verified(catalog: &Catalog, path: &Path) -> Result<Self, LockError> {
        if !catalog.servers().any(|server| server.enabled) {
            return Ok(Self::default());
        }
        let lock = Self::read(path)?.ok_or_else(|| LockError::Missing(path.to_owned()))?;
        lock.verify(catalog)?;
        Ok(lock)
    }

    /// Loads whatever the lockfile has, for `require_lock = false`. A missing file is fine and
    /// entries that no longer match are dropped, but a broken file still refuses: that's a
    /// mistake to fix, not a lock nobody wrote.
    pub fn load_lenient(catalog: &Catalog, path: &Path) -> Result<Self, LockError> {
        let mut lock = Self::read(path)?.unwrap_or_default();
        lock.server.retain(|id, locked| {
            let matches = catalog.get(id).is_none_or(|server| {
                locked.package == server.package && locked.runtime == server.runtime
            });
            if !matches {
                tracing::warn!(server = %id, "ignoring a stale lockfile entry");
            }
            matches
        });
        Ok(lock)
    }

    fn read(path: &Path) -> Result<Option<Self>, LockError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(LockError::Read {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        let lock: Self = toml::from_str(&text).map_err(|source| LockError::Parse {
            path: path.to_owned(),
            source,
        })?;
        if lock.version != LOCK_VERSION {
            return Err(LockError::WrongVersion {
                path: path.to_owned(),
                found: lock.version,
            });
        }
        Ok(Some(lock))
    }

    fn verify(&self, catalog: &Catalog) -> Result<(), LockError> {
        for server in catalog.servers().filter(|server| server.enabled) {
            let locked = self.get(&server.id).ok_or_else(|| LockError::NotLocked {
                server: server.id.clone(),
            })?;
            if locked.package != server.package || locked.runtime != server.runtime {
                return Err(LockError::Stale {
                    server: server.id.clone(),
                });
            }
        }
        Ok(())
    }

    pub fn get(&self, server_id: &str) -> Option<&LockedServer> {
        self.server.get(server_id)
    }

    pub fn insert(&mut self, server_id: String, locked: LockedServer) {
        self.server.insert(server_id, locked);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CATALOG: &str = r#"
        [[server]]
        id = "demo"
        mode = "stdio"
        runtime = "bun"
        package = "demo@1.0.0"

        [[server]]
        id = "off"
        enabled = false
        mode = "stdio"
        runtime = "bun"
        package = "off@1.0.0"
    "#;

    fn locked(package: &str, runtime: RuntimeKind) -> LockedServer {
        LockedServer {
            package: package.to_owned(),
            runtime,
            lock: "lock contents\nwith \"quotes\"\n".to_owned(),
        }
    }

    fn write(lock: &CatalogLock) -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("catalog.lock");
        std::fs::write(&path, toml::to_string_pretty(lock).unwrap()).unwrap();
        (directory, path)
    }

    #[test]
    fn the_path_sits_next_to_the_catalog() {
        assert_eq!(
            CatalogLock::path_for(Path::new("/etc/mcp-hub/catalog.toml")),
            Path::new("/etc/mcp-hub/catalog.lock")
        );
        assert_eq!(
            CatalogLock::path_for(Path::new("../config/catalog.example.toml")),
            Path::new("../config/catalog.example.lock")
        );
    }

    #[test]
    fn a_matching_lock_round_trips_and_disabled_servers_need_none() {
        let catalog = Catalog::parse(CATALOG).unwrap();
        let mut lock = CatalogLock::default();
        lock.insert("demo".to_owned(), locked("demo@1.0.0", RuntimeKind::Bun));
        let (_directory, path) = write(&lock);
        assert_eq!(CatalogLock::load_verified(&catalog, &path).unwrap(), lock);
    }

    #[test]
    fn a_missing_lock_refuses_unless_nothing_is_enabled() {
        let path = Path::new("/nonexistent/catalog.lock");
        let catalog = Catalog::parse(CATALOG).unwrap();
        let error = CatalogLock::load_verified(&catalog, path).unwrap_err();
        assert!(matches!(error, LockError::Missing(_)));
        assert!(error.to_string().contains("mcp-hub catalog lock"));

        let nothing_enabled = Catalog::parse("").unwrap();
        assert!(CatalogLock::load_verified(&nothing_enabled, path).is_ok());
    }

    #[test]
    fn an_unlocked_or_changed_server_refuses() {
        let catalog = Catalog::parse(CATALOG).unwrap();
        let (_directory, path) = write(&CatalogLock::default());
        assert!(matches!(
            CatalogLock::load_verified(&catalog, &path),
            Err(LockError::NotLocked { .. })
        ));

        for stale in [
            locked("demo@1.0.1", RuntimeKind::Bun),
            locked("demo@1.0.0", RuntimeKind::Node),
        ] {
            let mut lock = CatalogLock::default();
            lock.insert("demo".to_owned(), stale);
            let (_directory, path) = write(&lock);
            assert!(matches!(
                CatalogLock::load_verified(&catalog, &path),
                Err(LockError::Stale { .. })
            ));
        }
    }

    #[test]
    fn lenient_loading_accepts_a_missing_file_and_drops_stale_entries() {
        let catalog = Catalog::parse(CATALOG).unwrap();
        let missing = Path::new("/nonexistent/catalog.lock");
        assert_eq!(
            CatalogLock::load_lenient(&catalog, missing).unwrap(),
            CatalogLock::default()
        );

        let mut lock = CatalogLock::default();
        lock.insert("demo".to_owned(), locked("demo@1.0.0", RuntimeKind::Bun));
        let (_directory, path) = write(&lock);
        assert_eq!(CatalogLock::load_lenient(&catalog, &path).unwrap(), lock);

        let mut stale = CatalogLock::default();
        stale.insert("demo".to_owned(), locked("demo@0.9.0", RuntimeKind::Bun));
        let (_directory, path) = write(&stale);
        assert!(
            CatalogLock::load_lenient(&catalog, &path)
                .unwrap()
                .get("demo")
                .is_none()
        );
    }

    #[test]
    fn lenient_loading_still_refuses_a_broken_file() {
        let catalog = Catalog::parse(CATALOG).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("catalog.lock");
        std::fs::write(&path, "not toml [").unwrap();
        assert!(matches!(
            CatalogLock::load_lenient(&catalog, &path),
            Err(LockError::Parse { .. })
        ));
    }

    #[test]
    fn a_broken_or_future_lock_refuses() {
        let catalog = Catalog::parse(CATALOG).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("catalog.lock");
        std::fs::write(&path, "not toml [").unwrap();
        assert!(matches!(
            CatalogLock::load_verified(&catalog, &path),
            Err(LockError::Parse { .. })
        ));
        std::fs::write(&path, "version = 99\n").unwrap();
        assert!(matches!(
            CatalogLock::load_verified(&catalog, &path),
            Err(LockError::WrongVersion { found: 99, .. })
        ));
    }
}
