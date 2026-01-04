# SKCC Support Design

## Overview

Add SKCC (Straight Key Century Club) roster support to qrqcrew-notes-daemon, publishing to ditdit.club.

## Data Source

- **URL:** https://skccgroup.com/membership_data/membership_roster.php
- **Format:** HTML table (not CSV like existing sources)
- **Key fields:** SKCC # (with achievement suffixes), Callsign

### SKCC Number Format

Member numbers include achievement suffixes:
- Plain number: basic member (e.g., `1`)
- `C` suffix: Centurion (e.g., `2C`)
- `S` suffix: Senator (e.g., `3S`)
- `T` suffix: Tribune

### Filtering

- Exclude Silent Key entries (callsigns ending with `/SK`)
- Validate callsigns with existing regex pattern

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                     Organization Config                      │
│  source_type = "html_table" | "csv"  (new field)            │
└─────────────────────────────────────────────────────────────┘
                              │
              ┌───────────────┴───────────────┐
              ▼                               ▼
      ┌──────────────┐                ┌──────────────┐
      │  CsvFetcher  │                │  HtmlFetcher │
      │  (existing)  │                │   (new)      │
      └──────────────┘                └──────────────┘
              │                               │
              └───────────────┬───────────────┘
                              ▼
                    ┌──────────────────┐
                    │  Vec<Member>     │
                    └──────────────────┘
                              │
                              ▼
                    ┌──────────────────┐
                    │  NotesGenerator  │
                    │  (unchanged)     │
                    └──────────────────┘
```

## Output

- **File:** `skcc_members.txt`
- **Repo:** `jsvana/ditdit.club`
- **Emoji:** 🔑
- **Label:** SKCC
- **Format:** `CALLSIGN 🔑 SKCC #2C`

## Config Example

```toml
[[organizations]]
name = "skcc"
enabled = true
source_type = "html_table"
roster_url = "https://skccgroup.com/membership_data/membership_roster.php"
callsign_column_index = 1
number_column_index = 0
emoji = "🔑"
label = "SKCC"
output_file = "skcc_members.txt"

[organizations.github]
token = "${DITDIT_GITHUB_TOKEN}"
owner = "jsvana"
repo = "ditdit.club"
```

## Code Changes

### Modified Files

1. **`src/lib.rs`** - Export `html_fetcher` module
2. **`src/csv_fetcher.rs`** - Change `Member.qc_number: u32` → `member_id: String`
3. **`src/notes_generator.rs`** - Use `member_id` string for display
4. **`src/config.rs`** - Add fields:
   - `source_type: Option<String>` (default "csv")
   - `callsign_column_index: Option<usize>`
   - `number_column_index: Option<usize>`
5. **`src/main.rs`** - Dispatch to correct fetcher based on `source_type`
6. **`Cargo.toml`** - Add `scraper` dependency

### New Files

1. **`src/html_fetcher.rs`** - HTML table scraper for SKCC roster

## Dependencies

- `scraper` crate for HTML parsing
