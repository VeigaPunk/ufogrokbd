//! ufo-cli — pure Rust local rover (mailbox substrate = JSONL file)
//! No Hub HTTP required for core path. Nuke all non-Rust. LTS deps only.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::sleep;
use uuid::Uuid;

const APP_DIR: &str = ".ufo";
const ROVERS_FILE: &str = "rovers.json";
const MAILBOX_FILE: &str = "mailbox.jsonl";

#[derive(Parser, Debug)]
#[command(name = "ufo", about = "UFO local rover CLI (pure Rust, local JSONL mailbox)")]
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RoverEntry {
    id: String,
    name: String,
    units: u32,
    tags: Vec<String>,
    enrolled_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Operation {
    id: String,
    title: String,
    pilot_cmd: String,
    status: String, // queued | running | done | failed
    created_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
}

fn app_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("no home dir")?;
    let d = home.join(APP_DIR);
    fs::create_dir_all(&d)?;
    Ok(d)
}

fn rovers_path() -> Result<PathBuf> {
    Ok(app_dir()?.join(ROVERS_FILE))
}

fn mailbox_path() -> Result<PathBuf> {
    Ok(app_dir()?.join(MAILBOX_FILE))
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
    let p = rovers_path()?;
    let s = serde_json::to_string_pretty(rovers)?;
    fs::write(p, s)?;
    Ok(())
}

fn append_op(op: &Operation) -> Result<()> {
    let p = mailbox_path()?;
    let mut f = OpenOptions::new().create(true).append(true).open(p)?;
    writeln!(f, "{}", serde_json::to_string(op)?)?;
    Ok(())
}

fn load_mailbox() -> Result<Vec<Operation>> {
    let p = mailbox_path()?;
    if !p.exists() {
        return Ok(vec![]);
    }
    let s = fs::read_to_string(&p)?;
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

fn rewrite_mailbox(ops: &[Operation]) -> Result<()> {
    let p = mailbox_path()?;
    let mut f = OpenOptions::new().create(true).write(true).truncate(true).open(p)?;
    for op in ops {
        writeln!(f, "{}", serde_json::to_string(op)?)?;
    }
    Ok(())
}

async fn execute_op(op: &Operation, work_root: &Path) -> Result<()> {
    let op_dir = work_root.join(&op.id);
    fs::create_dir_all(&op_dir)?;
    println!("[ufo] running op {} in {:?}", op.id, op_dir);

    // Minimal pilot: shell out the pilot_cmd inside the work dir
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
        anyhow::bail!("pilot exited {:?}", status.code())
    }
}

async fn rover_loop(poll_secs: u64) -> Result<()> {
    let work_root = app_dir()?.join("work");
    fs::create_dir_all(&work_root)?;
    println!("[ufo] rover loop started, mailbox={:?}, poll={}s", mailbox_path()?, poll_secs);

    loop {
        let mut ops = load_mailbox()?;
        let mut changed = false;
        for op in ops.iter_mut() {
            if op.status == "queued" {
                op.status = "running".into();
                changed = true;
                if let Err(e) = execute_op(op, &work_root).await {
                    eprintln!("[ufo] op {} failed: {e:#}", op.id);
                    op.status = "failed".into();
                } else {
                    op.status = "done".into();
                }
                op.finished_at = Some(Utc::now());
                changed = true;
            }
        }
        if changed {
            rewrite_mailbox(&ops)?;
        }
        sleep(Duration::from_secs(poll_secs)).await;
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
        Commands::Start { headless: _, poll_secs } => {
            let rovers = load_rovers()?;
            if rovers.is_empty() {
                anyhow::bail!("no rovers enrolled — run `ufo enroll` first");
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
                println!("{} | {} | {} | {}", op.id, op.status, op.title, op.pilot_cmd);
            }
        }
    }
    Ok(())
}
