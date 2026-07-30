# ufo-cli-beads

Fork of ufo-cli that uses **beads** (`bd`) as the mailbox substrate.

Operations are beads issues. Rover claims via `bd ready` / `bd update --status=in_progress`, runs pilot, closes with `bd close`.

## Prerequisites
- `bd` on PATH (https://github.com/steveyegge/beads or gastownhall/beads)
- Project with `bd init`

## Install
```bash
cargo install --path . --locked
```

## Usage
```bash
ufo enroll
cd /your/project   # already bd init'ed
ufo push --title "do the thing" --pilot-cmd "cargo test"
ufo start --project .
```
