# ufo-cli (VeigaPunk pure-Rust fork)

Local rover for UFO-style agent orchestration. **Pure Rust. Zero GitHub Actions. Local JSONL mailbox substrate.**

## Install (LTS deps)

```bash
# Requires Rust stable (1.75+ recommended)
cargo install --path . --locked
```

Deps are pinned to LTS-compatible versions in Cargo.toml.

## Quick start

```bash
ufo enroll --name my-rover --units 2
ufo push --title "test" --pilot-cmd "echo hello && date"
ufo start --poll-secs 2
# in another terminal
ufo mailbox
```

Mailbox lives at `~/.ufo/mailbox.jsonl`. Worktrees under `~/.ufo/work/<op-id>`.

No Hub, no network required for the core loop.
