# QRQ Crew Callsign Notes Daemon

A Rust daemon that generates [Ham2K PoLo](https://polo.ham2k.com/) callsign notes from the QRQ Crew roster.

## Features

- Fetches roster from Google Sheets CSV export
- Validates callsigns with regex pattern
- Generates notes with format: `CALLSIGN ⚓ QRQ Crew #N`
- Commits changes to GitHub repository (only when content changes)
- Supports daemon mode with configurable sync interval
- Supports `--dry-run` and `--once` CLI flags

## Installation

```bash
cargo build --release
```

## Configuration

Copy `config/config.example.toml` to `config.toml` and edit:

```toml
roster_url = "https://docs.google.com/spreadsheets/d/.../pub?output=csv"

[github]
token = "${GITHUB_TOKEN}"  # Use environment variable
owner = "your-username"
repo = "your-repo"
branch = "main"
file_path = "callsign-notes.txt"
commit_author_name = "QRQ Crew Bot"
commit_author_email = "bot@example.com"

[daemon]
sync_interval_secs = 3600  # 1 hour
run_once = false

[output]
emoji = "⚓"
```

Set your GitHub token:
```bash
export GITHUB_TOKEN="your-token-here"
```

## Usage

```bash
# Run once and exit
./target/release/qrqcrew-notes-daemon --once

# Dry run (doesn't commit to GitHub)
./target/release/qrqcrew-notes-daemon --dry-run --once

# Run as daemon (syncs every sync_interval_secs)
./target/release/qrqcrew-notes-daemon
```

## Output Format

```
# QRQ Crew Callsign Notes for Ham2K PoLo
# Generated: 2025-12-19 00:00:00 UTC
# https://qrqcrew.club
# Do not edit manually - this file is auto-generated

K4MW ⚓ QRQ Crew #1
WN7JT ⚓ QRQ Crew #2
KI7QCF ⚓ QRQ Crew #3
...
```

## License

MIT
