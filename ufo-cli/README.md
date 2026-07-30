# ufo-cli

Local rover for UFO-style orchestration.

Pure Rust. Local JSONL mailbox substrate. Pilot commands run through POSIX `sh`.

## Install

### Cargo

```bash
cargo install --path . --locked
```

### Arch

From the repo root:

```bash
makepkg -si
```

## Use

```bash
ufo enroll --name my-rover --units 2
ufo push --title test --pilot-cmd "echo hello && date"
ufo start --poll-secs 2
ufo mailbox
```

Mailbox: `~/.ufo/mailbox.jsonl`
Workdirs: `~/.ufo/work/<op-id>`
