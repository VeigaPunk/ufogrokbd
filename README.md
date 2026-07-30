# ufogrokbd

Pure-Rust UFO local rover CLIs.

- **ufo-cli/** — local JSONL mailbox substrate (`~/.ufo/mailbox.jsonl`)
- **ufo-cli-beads/** — beads (`bd`) as mailbox substrate

**Auth connection cloned from OpenCode**: both binaries load credentials from `~/.local/share/opencode/auth.json` (preferred) or `~/.ufo/auth.json`. Use `ufo auth list` / `ufo auth status`.

No GitHub Actions. Exact LTS dependency pins. Offline-first core loop.

## Quick start (local mailbox)

```bash
cd ufo-cli
cargo install --path . --locked
ufo enroll --name rover-1
ufo auth status
ufo push --title test --pilot-cmd "echo hello"
ufo start
```

## Quick start (beads)

```bash
# requires `bd` on PATH + `bd init` in target project
cd ufo-cli-beads
cargo install --path . --locked
ufo enroll
ufo auth list
ufo push --title "do X" --pilot-cmd "cargo test" --project /path/to/project
ufo start --project /path/to/project
```

Godspeed.
