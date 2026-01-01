# Multi-Organization Support Design

## Overview

Add support for multiple amateur radio organizations (QRQ Crew, CWops, etc.) to the callsign notes daemon. Each organization gets its own output file committed to GitHub.

## Configuration

The config changes from a single `roster_url` to an array of organizations:

```toml
[[organizations]]
name = "qrqcrew"
enabled = true
roster_url = "https://docs.google.com/spreadsheets/d/.../pub?output=csv"
callsign_column = "call"
number_column = "qc #"
skip_rows = 0
emoji = "⚓"
label = "QRQ Crew"
output_file = "qrqcrew-notes.txt"

[[organizations]]
name = "cwops"
enabled = true
roster_url = "https://docs.google.com/spreadsheets/d/1Ew8b1WAorFRCixGRsr031atxmS0SsycvmOczS_fDqzc/export?format=csv"
callsign_column = "Callsign"
number_column = "Number"
skip_rows = 6
emoji = "🎹"
label = "CWops"
output_file = "cwops-notes.txt"

[github]
token = "${GITHUB_TOKEN}"
owner = "jsvana"
repo = "qrqcrew-site"
branch = "main"

[daemon]
sync_interval_secs = 3600
run_once = false
```

### Organization Fields

| Field | Description |
|-------|-------------|
| `name` | Identifier for logging |
| `enabled` | Toggle org on/off without removing config |
| `roster_url` | Google Sheets CSV export URL |
| `callsign_column` | Exact header name for callsign column |
| `number_column` | Exact header name for member number column |
| `skip_rows` | Metadata rows to skip before header |
| `emoji` | Emoji prefix in output |
| `label` | Organization name in output |
| `output_file` | GitHub file path for this org |

## Code Changes

### Modified Files

1. **`config.rs`**
   - Add `Organization` struct
   - Replace `roster_url` with `organizations: Vec<Organization>`
   - Remove `output.emoji` (now per-org)

2. **`csv_fetcher.rs`**
   - Accept org config instead of just URL
   - Use explicit column names (no auto-detection)
   - Skip N rows before looking for header

3. **`notes_generator.rs`**
   - Accept org config for formatting
   - Use org's emoji and label in output

4. **`main.rs`**
   - Loop over enabled organizations
   - Independent error handling per org

### New Struct

```rust
pub struct Organization {
    pub name: String,
    pub enabled: bool,
    pub roster_url: String,
    pub callsign_column: String,
    pub number_column: String,
    pub skip_rows: usize,
    pub emoji: String,
    pub label: String,
    pub output_file: String,
}
```

## Output Format

Each org generates a separate file with consistent format:

```
# CWops Callsign Notes for Ham2K PoLo
# Generated: 2026-01-01 12:00:00 UTC
# https://cwops.org
# Do not edit manually - this file is auto-generated

4X4NJ 🎹 CWops #275
K4MW 🎹 CWops #1
W6JSV 🎹 CWops #1234
```

## Error Handling

- Each organization syncs independently
- If one org fails, others still sync
- Errors logged with org name prefix
- Retries per org (existing 3-attempt logic)

## Migration

Breaking change - old config format not supported. Update steps:
1. Replace config.toml with new format
2. Deploy new daemon

No backwards compatibility layer (small personal project).
