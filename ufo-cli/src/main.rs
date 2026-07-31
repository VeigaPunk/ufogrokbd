//! ufo-cli — pure Rust local rover (mailbox substrate = JSONL file)
//! Shell pilots intentionally use POSIX `sh`; the implementation stays Rust-only.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Component;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::sleep;
use ufo_auth as auth;
use uuid::Uuid;

const APP_DIR: &str = ".ufo";
const ROVERS_FILE: &str = "rovers.json";
const MAILBOX_FILE: &str = "mailbox.jsonl";

#[derive(Parser, Debug)]
#[command(
    name = "ufo",
    about = "UFO local rover CLI (pure Rust, local JSONL mailbox)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Enroll a local rover (stores identity under ~/.ufo)
    Enroll {
        #[arg(long, default_value = "local")]
        name: String,
        #[arg(long, default_value_t = 1)]
        units: u32,
        #[arg(long)]
        tags: Vec<String>,
    },
    /// Start the rover loop: pull ops from local mailbox, execute in worktrees
    Start {
        #[arg(long)]
        headless: bool,
        #[arg(long, default_value_t = 2)]
        poll_secs: u64,
    },
    /// Push a synthetic operation into the local mailbox (for testing)
    Push {
        #[arg(long)]
        title: String,
        #[arg(long, default_value = "echo 'hello from pilot'")]
        pilot_cmd: String,
    },
    /// List mailbox contents
    Mailbox,
    /// Auth (cloned from OpenCode auth connection)
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
}

#[derive(Subcommand, Debug)]
enum AuthAction {
    /// List providers from OpenCode ~/.local/share/opencode/auth.json or ~/.ufo/auth.json
    List,
    /// Show which store is active
    Status,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RoverEntry {
    id: String,
    name: String,
    units: u32,
    tags: Vec<String>,
    #[serde(with = "rfc3339_datetime")]
    enrolled_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Operation {
    id: String,
    title: String,
    pilot_cmd: String,
    status: String,
    #[serde(with = "rfc3339_datetime")]
    created_at: DateTime<Utc>,
    #[serde(with = "rfc3339_datetime::option")]
    finished_at: Option<DateTime<Utc>>,
}

struct MailboxLock(File);

impl Drop for MailboxLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn set_private_mode(path: &Path, dir: bool) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = if dir { 0o700 } else { 0o600 };
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(mode);
        fs::set_permissions(path, perms)?;
    }
    let _ = (path, dir);
    Ok(())
}

fn ensure_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    set_private_mode(path, true)?;
    Ok(())
}

fn ensure_private_file(path: &Path) -> Result<()> {
    if path.exists() {
        set_private_mode(path, false)?;
    }
    Ok(())
}

fn app_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("no home dir")?;
    let d = home.join(APP_DIR);
    ensure_no_symlink_components(&d)?;
    ensure_private_dir(&d)?;
    Ok(d)
}

fn ensure_no_symlink_components(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => current.push(component.as_os_str()),
            Component::CurDir => continue,
            Component::ParentDir => bail!("unsafe path component in {:?}", path),
            Component::Normal(seg) => current.push(seg),
        }
        if let Ok(meta) = fs::symlink_metadata(&current) {
            if meta.file_type().is_symlink() {
                bail!("unsafe symlink component in {:?}", path);
            }
        }
    }
    Ok(())
}

fn rovers_path() -> Result<PathBuf> {
    let path = app_dir()?.join(ROVERS_FILE);
    ensure_no_symlink_components(&path)?;
    Ok(path)
}

fn mailbox_path() -> Result<PathBuf> {
    let path = app_dir()?.join(MAILBOX_FILE);
    ensure_no_symlink_components(&path)?;
    Ok(path)
}

fn mailbox_lock_path(path: &Path) -> PathBuf {
    let mut lock = path.as_os_str().to_os_string();
    lock.push(".lock");
    PathBuf::from(lock)
}

fn acquire_mailbox_lock(path: &Path, shared: bool) -> Result<MailboxLock> {
    let lock_path = mailbox_lock_path(path);
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    if shared {
        file.lock_shared()?;
    } else {
        file.lock_exclusive()?;
    }
    Ok(MailboxLock(file))
}

mod rfc3339_datetime {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S>(
        value: &DateTime<Utc>,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_rfc3339())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> std::result::Result<DateTime<Utc>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = DateTime<Utc>;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an RFC3339 string or unix timestamp")
            }

            fn visit_i64<E>(self, value: i64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                DateTime::<Utc>::from_timestamp(value, 0)
                    .ok_or_else(|| E::custom("invalid unix timestamp"))
            }

            fn visit_u64<E>(self, value: u64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Self::visit_i64(self, value as i64)
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                DateTime::parse_from_rfc3339(value)
                    .map(|dt| dt.with_timezone(&Utc))
                    .map_err(E::custom)
            }
        }

        deserializer.deserialize_any(Visitor)
    }

    pub mod option {
        use super::*;
        use serde::{Deserialize, Deserializer, Serializer};

        pub fn serialize<S>(
            value: &Option<DateTime<Utc>>,
            serializer: S,
        ) -> std::result::Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            match value {
                Some(dt) => serializer.serialize_some(&dt.to_rfc3339()),
                None => serializer.serialize_none(),
            }
        }

        pub fn deserialize<'de, D>(
            deserializer: D,
        ) -> std::result::Result<Option<DateTime<Utc>>, D::Error>
        where
            D: Deserializer<'de>,
        {
            let raw = Option::<serde_json::Value>::deserialize(deserializer)?;
            match raw {
                None => Ok(None),
                Some(serde_json::Value::String(s)) => DateTime::parse_from_rfc3339(&s)
                    .map(|dt| Some(dt.with_timezone(&Utc)))
                    .map_err(serde::de::Error::custom),
                Some(serde_json::Value::Number(n)) => n
                    .as_i64()
                    .and_then(|ts| DateTime::<Utc>::from_timestamp(ts, 0))
                    .map(Some)
                    .ok_or_else(|| serde::de::Error::custom("invalid unix timestamp")),
                Some(other) => Err(serde::de::Error::custom(format!(
                    "unexpected datetime value: {other}"
                ))),
            }
        }
    }
}

struct TempWrite {
    path: PathBuf,
    committed: bool,
}

impl Drop for TempWrite {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn atomic_write_string(path: &Path, contents: &str) -> Result<()> {
    let parent = path.parent().context("missing parent directory")?;
    fs::create_dir_all(parent)?;

    let tmp_path = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(OsStr::to_str).unwrap_or("ufo"),
        Uuid::new_v4()
    ));
    let mut cleanup = TempWrite {
        path: tmp_path.clone(),
        committed: false,
    };

    {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp_path)?;
        file.write_all(contents.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
    }
    set_private_mode(&tmp_path, false)?;
    fs::rename(&tmp_path, path)?;

    let dir = OpenOptions::new().read(true).open(parent)?;
    dir.sync_all()?;
    ensure_private_file(path)?;
    cleanup.committed = true;
    Ok(())
}

fn load_rovers() -> Result<Vec<RoverEntry>> {
    let p = rovers_path()?;
    if !p.exists() {
        return Ok(vec![]);
    }
    let s = fs::read_to_string(&p)?;
    Ok(serde_json::from_str(&s).unwrap_or_default())
}

fn save_rovers(rovers: &[RoverEntry]) -> Result<()> {
    atomic_write_string(&rovers_path()?, &serde_json::to_string_pretty(rovers)?)
}

fn read_mailbox(path: &Path) -> Result<Vec<Operation>> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let mut s = String::new();
    File::open(path)?.read_to_string(&mut s)?;
    let mut ops = Vec::new();
    for line in s.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(op) = serde_json::from_str::<Operation>(line) {
            ops.push(op);
        }
    }
    Ok(ops)
}

fn load_mailbox_from(path: &Path) -> Result<Vec<Operation>> {
    let _lock = acquire_mailbox_lock(path, true)?;
    read_mailbox(path)
}

fn append_op_to(path: &Path, op: &Operation) -> Result<()> {
    let _lock = acquire_mailbox_lock(path, false)?;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", serde_json::to_string(op)?)?;
    file.flush()?;
    file.sync_all()?;
    ensure_private_file(path)?;
    Ok(())
}

fn write_mailbox_locked(path: &Path, ops: &[Operation]) -> Result<()> {
    let mut body = String::new();
    for op in ops {
        body.push_str(&serde_json::to_string(op)?);
        body.push('\n');
    }
    atomic_write_string(path, &body)
}

fn claim_next_mailbox_op(path: &Path) -> Result<Option<Operation>> {
    let _lock = acquire_mailbox_lock(path, false)?;
    let mut ops = read_mailbox(path)?;
    let Some(index) = ops.iter().position(|op| op.status == "queued") else {
        return Ok(None);
    };
    if !is_safe_path_component(&ops[index].id) {
        bail!("unsafe op id: {}", ops[index].id);
    }
    ops[index].status = "running".to_string();
    let claimed = ops[index].clone();
    write_mailbox_locked(path, &ops)?;
    Ok(Some(claimed))
}

fn finalize_mailbox_op(
    path: &Path,
    op_id: &str,
    status: &str,
    finished_at: Option<DateTime<Utc>>,
) -> Result<bool> {
    let _lock = acquire_mailbox_lock(path, false)?;
    let mut ops = read_mailbox(path)?;
    let mut changed = false;
    for op in &mut ops {
        if op.id == op_id {
            op.status = status.to_string();
            op.finished_at = finished_at;
            changed = true;
            break;
        }
    }
    if changed {
        write_mailbox_locked(path, &ops)?;
    }
    Ok(changed)
}

fn load_mailbox() -> Result<Vec<Operation>> {
    let path = mailbox_path()?;
    load_mailbox_from(&path)
}

fn append_op(op: &Operation) -> Result<()> {
    let path = mailbox_path()?;
    append_op_to(&path, op)
}

async fn execute_op(op: &Operation, work_root: &Path) -> Result<()> {
    ensure_safe_path_component(&op.id)?;
    let op_dir = work_root.join(&op.id);
    ensure_no_symlink_components(&op_dir)?;
    fs::create_dir_all(&op_dir)?;
    set_private_mode(&op_dir, true)?;
    println!("[ufo] running op {} in {:?}", op.id, op_dir);

    let status = Command::new("sh")
        .arg("-c")
        .arg(&op.pilot_cmd)
        .current_dir(&op_dir)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await
        .context("pilot spawn failed")?;

    if status.success() {
        println!("[ufo] op {} done", op.id);
        Ok(())
    } else {
        bail!("pilot exited {:?}", status.code())
    }
}

async fn rover_loop(poll_secs: u64) -> Result<()> {
    let work_root = app_dir()?.join("work");
    ensure_no_symlink_components(&work_root)?;
    fs::create_dir_all(&work_root)?;
    set_private_mode(&work_root, true)?;
    println!(
        "[ufo] rover loop started, mailbox={:?}, poll={}s",
        mailbox_path()?,
        poll_secs
    );

    loop {
        let mailbox = mailbox_path()?;
        match claim_next_mailbox_op(&mailbox)? {
            Some(op) => {
                let outcome = match execute_op(&op, &work_root).await {
                    Ok(()) => "done",
                    Err(e) => {
                        eprintln!("[ufo] op {} failed: {e:#}", op.id);
                        "failed"
                    }
                };
                let finished_at = Some(Utc::now());
                if !finalize_mailbox_op(&mailbox, &op.id, outcome, finished_at)? {
                    eprintln!("[ufo] finalize lost op {}", op.id);
                }
            }
            None => sleep(Duration::from_secs(poll_secs)).await,
        }
    }
}

fn is_safe_path_component(id: &str) -> bool {
    if id.is_empty() || id == "." || id == ".." {
        return false;
    }
    let path = Path::new(id);
    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(seg)), None) => seg == OsStr::new(id),
        _ => false,
    }
}

fn ensure_safe_path_component(id: &str) -> Result<()> {
    if is_safe_path_component(id) {
        Ok(())
    } else {
        bail!("unsafe op id: {id}")
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Commands::Enroll { name, units, tags } => {
            let mut rovers = load_rovers()?;
            let entry = RoverEntry {
                id: Uuid::new_v4().to_string(),
                name,
                units,
                tags,
                enrolled_at: Utc::now(),
            };
            println!("[ufo] enrolled rover id={} name={}", entry.id, entry.name);
            rovers.push(entry);
            save_rovers(&rovers)?;
        }
        Commands::Start {
            headless: _,
            poll_secs,
        } => {
            let rovers = load_rovers()?;
            if rovers.is_empty() {
                bail!("no rovers enrolled — run `ufo enroll` first");
            }
            println!("[ufo] {} rover(s) loaded", rovers.len());
            rover_loop(poll_secs).await?;
        }
        Commands::Push { title, pilot_cmd } => {
            let op = Operation {
                id: Uuid::new_v4().to_string(),
                title,
                pilot_cmd,
                status: "queued".into(),
                created_at: Utc::now(),
                finished_at: None,
            };
            append_op(&op)?;
            println!("[ufo] pushed op id={} title={}", op.id, op.title);
        }
        Commands::Mailbox => {
            let ops = load_mailbox()?;
            for op in ops {
                println!(
                    "{} | {} | {} | {}",
                    op.id, op.status, op.title, op.pilot_cmd
                );
            }
        }
        Commands::Auth { action } => match action {
            AuthAction::List => {
                let snapshot = auth::load_auth()?;
                let list = snapshot.store.summaries(snapshot.source.clone());
                if list.is_empty() {
                    println!(
                        "[ufo] no providers (OpenCode auth: env, XDG_DATA_HOME/opencode/auth.json, then ~/.local/share/opencode/auth.json)"
                    );
                } else {
                    for item in list {
                        let oauth = item
                            .oauth
                            .as_ref()
                            .map(|oauth| {
                                format!(
                                    " expires={} account={} enterprise={}",
                                    oauth.expiry_state,
                                    oauth.account_id.as_deref().unwrap_or("-"),
                                    oauth.enterprise_url.as_deref().unwrap_or("-")
                                )
                            })
                            .unwrap_or_default();
                        println!(
                            "{}  ({} source={} policy={} metadata={}){}",
                            item.provider_id,
                            item.kind,
                            item.source,
                            item.policy,
                            if item.metadata_present {
                                "present"
                            } else {
                                "-"
                            },
                            oauth
                        );
                    }
                    if snapshot.malformed_entries > 0 {
                        println!(
                            "[ufo] skipped malformed entries: {}",
                            snapshot.malformed_entries
                        );
                    }
                }
            }
            AuthAction::Status => {
                let snapshot = auth::load_auth()?;
                println!("OpenCode source: {}", snapshot.source);
                println!("OpenCode file: {:?}", snapshot.resolved_path);
                println!("loaded providers: {}", snapshot.store.len());
                println!("usable providers: {}", snapshot.store.usable_count());
                println!("skipped malformed entries: {}", snapshot.malformed_entries);
            }
        },
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ufo-cli-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_op(id: &str, title: &str) -> Operation {
        Operation {
            id: id.to_string(),
            title: title.to_string(),
            pilot_cmd: "true".to_string(),
            status: "queued".to_string(),
            created_at: Utc::now(),
            finished_at: None,
        }
    }

    #[test]
    fn legacy_rfc3339_operation_timestamp_parses() {
        let op: Operation = serde_json::from_str(
            r#"{"id":"op-1","title":"legacy","pilot_cmd":"true","status":"queued","created_at":"2024-01-02T03:04:05Z","finished_at":null}"#,
        )
        .unwrap();
        assert_eq!(op.created_at.to_rfc3339(), "2024-01-02T03:04:05+00:00");
    }

    #[test]
    fn safe_path_component_rejects_traversal() {
        assert!(is_safe_path_component("issue-123"));
        assert!(!is_safe_path_component("../issue"));
        assert!(!is_safe_path_component("a/b"));
        assert!(!is_safe_path_component(".."));
    }

    #[test]
    fn mailbox_append_is_atomic_under_concurrency() {
        let dir = temp_dir();
        let path = dir.join("mailbox.jsonl");
        let barrier = Arc::new(Barrier::new(2));
        let op_a = sample_op("a", "alpha");
        let op_b = sample_op("b", "bravo");

        let t1_path = path.clone();
        let t1_barrier = barrier.clone();
        let t1_op = op_a.clone();
        let t1 = thread::spawn(move || {
            t1_barrier.wait();
            append_op_to(&t1_path, &t1_op)
        });

        let t2_path = path.clone();
        let t2_barrier = barrier.clone();
        let t2_op = op_b.clone();
        let t2 = thread::spawn(move || {
            t2_barrier.wait();
            append_op_to(&t2_path, &t2_op)
        });

        t1.join().unwrap().unwrap();
        t2.join().unwrap().unwrap();

        let ops = load_mailbox_from(&path).unwrap();
        assert_eq!(ops.len(), 2);
        assert!(ops.iter().any(|op| op.id == "a"));
        assert!(ops.iter().any(|op| op.id == "b"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn claim_next_is_exactly_once_and_finalize_preserves_appends() {
        let dir = temp_dir();
        let path = dir.join("mailbox.jsonl");
        let first = sample_op("first", "alpha");
        let second = sample_op("second", "bravo");
        let ops = vec![first.clone(), second.clone()];
        write_mailbox_locked(&path, &ops).unwrap();

        let barrier = Arc::new(Barrier::new(2));
        let left_path = path.clone();
        let left_barrier = barrier.clone();
        let left = thread::spawn(move || {
            left_barrier.wait();
            claim_next_mailbox_op(&left_path).unwrap()
        });

        let right_path = path.clone();
        let right_barrier = barrier.clone();
        let right = thread::spawn(move || {
            right_barrier.wait();
            claim_next_mailbox_op(&right_path).unwrap()
        });

        let claimed = [left.join().unwrap(), right.join().unwrap()];
        let claimed_ids: Vec<_> = claimed
            .into_iter()
            .filter_map(|op| op.map(|op| op.id))
            .collect();
        assert_eq!(claimed_ids.len(), 2);
        assert!(claimed_ids.contains(&"first".to_string()));
        assert!(claimed_ids.contains(&"second".to_string()));

        let queued = sample_op("third", "charlie");
        append_op_to(&path, &queued).unwrap();
        let status = claim_next_mailbox_op(&path).unwrap().expect("claim exists");
        assert_eq!(status.status, "running");
        append_op_to(&path, &sample_op("fourth", "delta")).unwrap();
        assert!(finalize_mailbox_op(&path, &status.id, "done", Some(Utc::now())).unwrap());

        let ops = load_mailbox_from(&path).unwrap();
        assert!(ops.iter().any(|op| op.id == "first"));
        assert!(ops.iter().any(|op| op.id == "second"));
        assert!(ops.iter().any(|op| op.id == "third"));
        assert!(ops.iter().any(|op| op.id == "fourth"));
        assert_eq!(
            ops.iter().find(|op| op.id == status.id).unwrap().status,
            "done"
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_components_are_rejected() {
        let dir = temp_dir();
        let real = dir.join("real");
        let link = dir.join("link");
        fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let unsafe_path = link.join("mailbox.jsonl");
        assert!(ensure_no_symlink_components(&unsafe_path).is_err());
        assert!(ensure_no_symlink_components(&real.join("mailbox.jsonl")).is_ok());

        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn private_modes_are_set_on_dir_and_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir();
        let private_dir = dir.join("private");
        ensure_private_dir(&private_dir).unwrap();
        let mode = fs::metadata(&private_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);

        let file = dir.join("secret.txt");
        atomic_write_string(&file, "secret").unwrap();
        let file_mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn atomic_rewrite_creates_complete_file() {
        let dir = temp_dir();
        let path = dir.join("mailbox.jsonl");
        let ops = vec![sample_op("x", "xray"), sample_op("y", "yankee")];

        write_mailbox_locked(&path, &ops).unwrap();
        let loaded = load_mailbox_from(&path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, "x");
        assert_eq!(loaded[1].id, "y");

        let _ = fs::remove_dir_all(dir);
    }
}
