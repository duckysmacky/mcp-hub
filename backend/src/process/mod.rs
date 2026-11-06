//! Everything needed to run a child safely. All MCP children are built here.

use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use nix::{
    sys::{
        prctl,
        signal::{Signal, killpg},
    },
    unistd::Pid,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, Command},
};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use crate::catalog::{ServerMode, ServerTemplate};

/// The only `PATH` a child ever sees; user values can't change it.
pub const GATEWAY_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

const MAX_TAIL_LINE_BYTES: usize = 64 * 1024;
const REDACTED: &str = "[redacted]";

pub type EnvValue = (String, Zeroizing<String>);

pub struct ChildSpec<'a> {
    /// `None` runs `script` itself: Python entry points and binary servers.
    pub runtime_binary: Option<&'a Path>,
    /// Python children write no bytecode: the packages directory is shared.
    pub is_python: bool,
    pub script: &'a Path,
    pub args: &'a [String],
    /// Must come from [`declared_process_env`], plus the hub's own port var.
    pub env: &'a [EnvValue],
    pub scratch_dir: &'a Path,
}

/// Builds a child `Command`: argv only, no inherited env, an empty working directory.
///
/// Call this and spawn from an async task, never from `spawn_blocking`:
/// `PR_SET_PDEATHSIG` fires when the *thread* that spawned the child exits, and
/// blocking-pool threads exit when idle, which would kill the child.
pub fn build_child_command(spec: &ChildSpec<'_>) -> Command {
    let mut command = match spec.runtime_binary {
        Some(binary) => {
            let mut command = Command::new(binary);
            command.arg(spec.script);
            command
        }
        None => Command::new(spec.script),
    };
    command
        .args(spec.args)
        .env_clear()
        .envs(
            spec.env
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
        )
        .env("PATH", GATEWAY_PATH)
        .env("HOME", spec.scratch_dir)
        .current_dir(spec.scratch_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    if spec.is_python {
        command.env("PYTHONDONTWRITEBYTECODE", "1");
    }
    // SAFETY: the closure runs between fork and exec, so it may only make
    // async-signal-safe calls. It makes one prctl syscall and allocates nothing.
    unsafe {
        command.pre_exec(|| prctl::set_pdeathsig(Signal::SIGKILL).map_err(std::io::Error::from));
    }
    command
}

/// Picks the values a child may have in its env: only vars the catalog still lists,
/// and in `http` mode never a var that is sent as a request header.
pub fn declared_process_env(template: &ServerTemplate, values: Vec<EnvValue>) -> Vec<EnvValue> {
    let header_vars: HashSet<&str> = match (template.mode, &template.http) {
        (ServerMode::Http, Some(http)) => http.header_vars(),
        _ => HashSet::new(),
    };
    values
        .into_iter()
        .filter(|(name, _)| {
            template.env_spec(name).is_some() && !header_vars.contains(name.as_str())
        })
        .collect()
}

/// The values output must be redacted against. Non-secret values stay: redacting a
/// flag set to `true` would blank that word everywhere in the status page's output.
pub fn secret_values(template: &ServerTemplate, values: &[EnvValue]) -> Vec<Zeroizing<String>> {
    values
        .iter()
        .filter(|(name, _)| template.env_spec(name).is_none_or(|spec| spec.secret))
        .map(|(_, value)| value.clone())
        .collect()
}

/// Kills a child's whole process group. Already-gone groups are not an error.
pub fn kill_group(process_id: u32) {
    if let Ok(raw_id) = i32::try_from(process_id) {
        let _ = killpg(Pid::from_raw(raw_id), Signal::SIGKILL);
    }
}

/// Creates a fresh, private scratch directory for one process.
pub async fn create_scratch_dir(root: &Path, name: &str) -> std::io::Result<PathBuf> {
    let directory = root.join(name);
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(&directory).await?;
    Ok(directory)
}

/// A bounded buffer of a child's most recent output. Holds only redacted text.
#[derive(Clone)]
pub struct OutputTail {
    bytes: Arc<Mutex<VecDeque<u8>>>,
    capacity: usize,
}

impl OutputTail {
    pub fn new(capacity: usize) -> Self {
        Self {
            bytes: Arc::new(Mutex::new(VecDeque::new())),
            capacity,
        }
    }

    fn push_line(&self, line: &str) {
        let mut bytes = self.bytes.lock().expect("no code panics holding this lock");
        bytes.extend(line.as_bytes());
        bytes.push_back(b'\n');
        let excess = bytes.len().saturating_sub(self.capacity);
        bytes.drain(..excess);
    }

    pub fn contents(&self) -> String {
        let bytes = self.bytes.lock().expect("no code panics holding this lock");
        String::from_utf8_lossy(&bytes.iter().copied().collect::<Vec<u8>>()).into_owned()
    }
}

/// Stops a process and its tasks, and remembers whether the hub asked for that.
/// The token also fires when the process's streams fail or end on their own.
#[derive(Clone, Default)]
pub struct ProcessStop {
    pub token: CancellationToken,
    requested_by_hub: Arc<AtomicBool>,
}

impl ProcessStop {
    pub fn request(&self) {
        self.requested_by_hub.store(true, Ordering::SeqCst);
        self.token.cancel();
    }

    pub fn requested_by_hub(&self) -> bool {
        self.requested_by_hub.load(Ordering::SeqCst)
    }
}

/// What the status page shows about an instance's processes. Outlives each process.
#[derive(Clone)]
pub struct InstanceOutput {
    pub tail: OutputTail,
    last_exit: Arc<Mutex<Option<String>>>,
}

impl InstanceOutput {
    pub fn new(tail_capacity: usize) -> Self {
        Self {
            tail: OutputTail::new(tail_capacity),
            last_exit: Arc::default(),
        }
    }

    pub fn last_exit(&self) -> Option<String> {
        self.last_exit
            .lock()
            .expect("no code panics holding this lock")
            .clone()
    }

    /// Owns a running child until it's gone: kills its group when `stop` fires, stops
    /// everything else if it exits on its own, then removes its scratch directory.
    pub async fn supervise_child(self, mut child: Child, stop: ProcessStop, scratch_dir: PathBuf) {
        let process_id = child.id();
        let status = tokio::select! {
            status = child.wait() => status,
            () = stop.token.cancelled() => {
                if let Some(process_id) = process_id {
                    kill_group(process_id);
                }
                child.wait().await
            }
        };
        stop.token.cancel();
        if let Some(process_id) = process_id {
            // Anything the child left behind in its group goes too.
            kill_group(process_id);
        }

        let reason = match status {
            _ if stop.requested_by_hub.load(Ordering::SeqCst) => "stopped by the hub".to_owned(),
            Ok(status) => status.to_string(),
            Err(error) => format!("could not get exit status: {error}"),
        };
        tracing::info!(reason, "child process ended");
        *self
            .last_exit
            .lock()
            .expect("no code panics holding this lock") = Some(reason);

        if let Err(error) = tokio::fs::remove_dir_all(&scratch_dir).await {
            tracing::warn!(%error, "could not remove a scratch directory");
        }
    }
}

/// Reads a child's output until EOF, storing redacted lines in `tail`.
///
/// It must keep reading whatever arrives, or the child blocks once the pipe fills,
/// so invalid UTF-8 and over-long lines are tolerated. Working line by line means a
/// secret can't escape redaction by being split across two reads.
pub async fn drain_output<R: AsyncRead + Unpin>(
    mut reader: R,
    tail: OutputTail,
    secrets: Arc<Vec<Zeroizing<String>>>,
) {
    let mut chunk = [0u8; 8192];
    let mut line = Zeroizing::new(Vec::new());
    let mut overflowed = false;
    loop {
        let read = match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        for &byte in &chunk[..read] {
            if byte == b'\n' {
                store_line(&line, &mut overflowed, &tail, &secrets);
                line.clear();
            } else if line.len() < MAX_TAIL_LINE_BYTES {
                line.push(byte);
            } else {
                overflowed = true;
            }
        }
    }
    if !line.is_empty() {
        store_line(&line, &mut overflowed, &tail, &secrets);
    }
}

fn store_line(
    line: &[u8],
    overflowed: &mut bool,
    tail: &OutputTail,
    secrets: &[Zeroizing<String>],
) {
    if std::mem::take(overflowed) {
        // The cut may have split a secret, so an over-long line is never stored.
        tail.push_line("[line too long, dropped]");
    } else {
        tail.push_line(&redact(&String::from_utf8_lossy(line), secrets));
    }
}

fn redact(line: &str, secrets: &[Zeroizing<String>]) -> String {
    secrets
        .iter()
        .filter(|secret| !secret.is_empty())
        .fold(line.to_owned(), |text, secret| {
            text.replace(secret.as_str(), REDACTED)
        })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use tokio::io::{AsyncWriteExt, duplex};

    use super::*;
    use crate::catalog::Catalog;

    fn value(name: &str, text: &str) -> EnvValue {
        (name.to_owned(), Zeroizing::new(text.to_owned()))
    }

    #[tokio::test]
    async fn python_entry_points_run_themselves_with_gateway_set_vars() {
        let scratch = tempfile::tempdir().unwrap();
        let script = scratch.path().join("entry.sh");
        std::fs::write(&script, "#!/bin/sh\necho \"args=$*\"\nenv\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let output = build_child_command(&ChildSpec {
            runtime_binary: None,
            is_python: true,
            script: &script,
            args: &["--flag".to_owned()],
            env: &[],
            scratch_dir: scratch.path(),
        })
        .output()
        .await
        .unwrap();

        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("args=--flag"), "{text}");
        assert!(text.contains("PYTHONDONTWRITEBYTECODE=1"), "{text}");
    }

    #[tokio::test]
    async fn binaries_run_themselves_and_get_no_python_vars() {
        let scratch = tempfile::tempdir().unwrap();
        let binary = scratch.path().join("server.sh");
        std::fs::write(&binary, "#!/bin/sh\necho \"args=$*\"\nenv\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();

        let output = build_child_command(&ChildSpec {
            runtime_binary: None,
            is_python: false,
            script: &binary,
            args: &["--flag".to_owned()],
            env: &[],
            scratch_dir: scratch.path(),
        })
        .output()
        .await
        .unwrap();

        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("args=--flag"), "{text}");
        assert!(!text.contains("PYTHON"), "{text}");
    }

    #[tokio::test]
    async fn child_sees_only_the_given_env_and_an_empty_cwd() {
        let scratch = tempfile::tempdir().unwrap();
        let script = scratch.path().join("script.sh");
        std::fs::write(&script, "env\npwd\nls -A\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let work = tempfile::tempdir().unwrap();

        let env = [value("ALLOWED", "yes")];
        let output = build_child_command(&ChildSpec {
            runtime_binary: Some(Path::new("/bin/sh")),
            is_python: false,
            script: &script,
            args: &[],
            env: &env,
            scratch_dir: work.path(),
        })
        .output()
        .await
        .unwrap();

        let text = String::from_utf8(output.stdout).unwrap();
        let names: HashSet<&str> = text
            .lines()
            .filter_map(|line| line.split_once('=').map(|(name, _)| name))
            .filter(|name| !["PWD", "OLDPWD", "_", "SHLVL"].contains(name))
            .collect();
        assert_eq!(names, HashSet::from(["ALLOWED", "PATH", "HOME"]));
        assert!(text.contains(&format!("HOME={}", work.path().display())));
        assert!(text.contains(&format!("PATH={GATEWAY_PATH}")));
        assert!(text.contains(&format!("\n{}\n", work.path().display())));
        assert!(
            text.ends_with(&format!("{}\n", work.path().display())),
            "cwd not empty"
        );
    }

    #[tokio::test]
    async fn child_gets_its_own_process_group_which_can_be_killed() {
        let work = tempfile::tempdir().unwrap();
        let script = work.path().join("sleep.sh");
        std::fs::write(&script, "sleep 30 & wait\n").unwrap();

        let mut child = build_child_command(&ChildSpec {
            runtime_binary: Some(Path::new("/bin/sh")),
            is_python: false,
            script: &script,
            args: &[],
            env: &[],
            scratch_dir: work.path(),
        })
        .spawn()
        .unwrap();

        let process_id = child.id().unwrap();
        let group = nix::unistd::getpgid(Some(Pid::from_raw(process_id as i32))).unwrap();
        assert_eq!(group.as_raw() as u32, process_id);

        kill_group(process_id);
        assert!(!child.wait().await.unwrap().success());
    }

    #[test]
    fn process_env_drops_unlisted_and_header_vars() {
        let catalog = Catalog::parse(
            r#"
            [[server]]
            id = "demo"
            mode = "http"
            runtime = "bun"
            package = "demo@1.0.0"

            [[server.env]]
            name = "BASE_URL"

            [[server.env]]
            name = "TOKEN"

            [server.http]
            port_env = "PORT"
            headers = { Authorization = "Bearer {TOKEN}" }
            "#,
        )
        .unwrap();

        let env = declared_process_env(
            catalog.get("demo").unwrap(),
            vec![
                value("BASE_URL", "https://example.com"),
                value("TOKEN", "secret"),
                value("REMOVED_FROM_CATALOG", "old"),
            ],
        );

        let names: Vec<&str> = env.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["BASE_URL"]);
    }

    #[tokio::test]
    async fn output_is_redacted_even_when_split_across_reads() {
        let (mut writer, reader) = duplex(64);
        let tail = OutputTail::new(1024);
        let secrets = Arc::new(vec![Zeroizing::new("hunter2".to_owned())]);
        let drain = tokio::spawn(drain_output(reader, tail.clone(), secrets));

        writer.write_all(b"token=hun").await.unwrap();
        writer.flush().await.unwrap();
        tokio::task::yield_now().await;
        writer.write_all(b"ter2 end\nlast").await.unwrap();
        drop(writer);
        drain.await.unwrap();

        assert_eq!(tail.contents(), "token=[redacted] end\nlast\n");
    }

    #[tokio::test]
    async fn tail_stays_bounded_and_survives_hostile_output() {
        let (mut writer, reader) = duplex(1024);
        let tail = OutputTail::new(64);
        let drain = tokio::spawn(drain_output(reader, tail.clone(), Arc::new(Vec::new())));

        writer.write_all(&[0xff, 0xfe, b'\n']).await.unwrap();
        writer
            .write_all(&vec![b'x'; MAX_TAIL_LINE_BYTES + 10])
            .await
            .unwrap();
        writer.write_all(b"\nafter\n").await.unwrap();
        drop(writer);
        drain.await.unwrap();

        let contents = tail.contents();
        assert!(contents.len() <= 64);
        assert!(
            contents.ends_with("[line too long, dropped]\nafter\n"),
            "{contents}"
        );
    }
}
