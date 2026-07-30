//! ufo-cli-beads — pure Rust rover using beads (bd) as mailbox substrate
//! Ops are beads issues of type "message" or "task". Ready work becomes runs.
//! Requires `bd` on PATH. Nuke non-Rust. LTS deps.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::sleep;
use uuid::Uuid;

const APP_DIR: &str = ".ufo";
const ROVERS_FILE: &str = "rovers.json";

#[derive(Parser, Debug)]
#[command(name = "ufo", about = "UFO rover (beads mailbox substrate)")]
struct Cli {
    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Enroll {
        #[arg(long, default_value = "local-beads")]
        name: String,
        #[arg(long, default_value_t = 1)]
        units: u32,
        #[arg(long)]
        tags: Vec<String>,
    },
    Start {
        #[arg(long, default_value_t = 3)]
        poll_secs: u64,
        /// Working directory that already has `bd init`
        #[arg(long, default_value = ".")]
        project: PathBuf,
    },
    /// Create a beads task that the rover will pick up
    Push {
        #[arg(long)]
        title: String,
        #[arg(long, default_value = "echo 'hello from beads pilot'")]
        pilot_cmd: String,
        #[arg(long, default_value = ".")]
        project: PathBuf,
    },
    Mailbox {
        #[arg(long, default_value = ".")]
        project: PathBuf,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RoverEntry {
    id: String,
    name: String,
    units: u32,
    tags: Vec<String>,
    enrolled_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct BdIssue {
    id: String,
    title: String,
    status: String,
    #[serde(default)]
    description: Option<String>,
}

fn app_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("no home")?;
    let d = home.join(APP_DIR);
    fs::create_dir_all(&d)?;
    Ok(d)
}

fn load_rovers() -> Result<Vec<RoverEntry>> {
    let p = app_dir()?.join(ROVERS_FILE);
    if !p.exists() {
        return Ok(vec![]);
    }
    Ok(serde_json::from_str(&fs::read_to_string(p)?)?)
}

fn save_rovers(r: &[RoverEntry]) -> Result<()> {
    fs::write(app_dir()?.join(ROVERS_FILE), serde_json::to_string_pretty(r)?)?;
    Ok(())
}

async fn bd_json(project: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("bd")
        .args(args)
        .arg("--json")
        .current_dir(project)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("bd not found on PATH — install beads first")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("bd failed: {err}");
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

async fn list_ready(project: &Path) -> Result<Vec<BdIssue>> {
    // Prefer ready; fall back to open tasks
    let raw = match bd_json(project, &["ready"]).await {
        Ok(s) if !s.trim().is_empty() && s.trim() != "[]" => s,
        _ => bd_json(project, &["list", "--status=open"]).await?,
    };
    let issues: Vec<BdIssue> = serde_json::from_str(&raw).unwrap_or_default();
    Ok(issues)
}

async fn claim_and_run(project: &Path, issue: &BdIssue, work_root: &Path) -> Result<()> {
    let _ = bd_json(project, &["update", &issue.id, "--status=in_progress"]).await;
    let op_dir = work_root.join(&issue.id);
    fs::create_dir_all(&op_dir)?;
    println!("[ufo-beads] claimed {} — {}", issue.id, issue.title);

    // Pilot cmd may be embedded in description as "pilot: <cmd>"
    let pilot = issue
        .description
        .as_deref()
        .and_then(|d| {
            d.lines()
                .find(|l| l.starts_with("pilot:"))
                .map(|l| l.trim_start_matches("pilot:").trim().to_string())
        })
        .unwrap_or_else(|| format!("echo 'executing beads issue {}'", issue.id));

    let status = Command::new("sh")
        .arg("-c")
        .arg(&pilot)
        .current_dir(&op_dir)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await?;

    if status.success() {
        let _ = bd_json(project, &["close", &issue.id]).await;
        println!("[ufo-beads] closed {}", issue.id);
    } else {
        eprintln!("[ufo-beads] pilot failed for {}", issue.id);
    }
    Ok(())
}

async fn rover_loop(project: PathBuf, poll_secs: u64) -> Result<()> {
    let work_root = app_dir()?.join("work-beads");
    fs::create_dir_all(&work_root)?;
    println!("[ufo-beads] loop on project={:?} poll={}s", project, poll_secs);

    loop {
        match list_ready(&project).await {
            Ok(issues) => {
                for iss in issues {
                    if let Err(e) = claim_and_run(&project, &iss, &work_root).await {
                        eprintln!("[ufo-beads] {e:#}");
                    }
                }
            }
            Err(e) => eprintln!("[ufo-beads] mailbox poll: {e:#}"),
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
            println!("[ufo-beads] enrolled {}", entry.id);
            rovers.push(entry);
            save_rovers(&rovers)?;
        }
        Commands::Start { poll_secs, project } => {
            if load_rovers()?.is_empty() {
                bail!("enroll first");
            }
            rover_loop(project, poll_secs).await?;
        }
        Commands::Push { title, pilot_cmd, project } => {
            let desc = format!("pilot: {pilot_cmd}");
            let out = Command::new("bd")
                .args(["create", &title, "-t", "task", "-p", "2", "--description", &desc, "--json"])
                .current_dir(&project)
                .output()
                .await
                .context("bd create")?;
            if !out.status.success() {
                bail!("bd create failed: {}", String::from_utf8_lossy(&out.stderr));
            }
            println!("[ufo-beads] pushed via beads:\n{}", String::from_utf8_lossy(&out.stdout));
        }
        Commands::Mailbox { project } => {
            let raw = bd_json(&project, &["list", "--status=open"]).await?;
            println!("{raw}");
        }
    }
    Ok(())
}
