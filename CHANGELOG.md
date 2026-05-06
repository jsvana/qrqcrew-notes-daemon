# Changelog

## [0.11.0] - 2026-05-06

### Changed
- QRZ lookup now resolves operators' current callsigns. Previously `lookup_nickname` extracted only `<fname>` from the QRZ XML response; we now extract `<call>` as well and use it as the authoritative current callsign. When the queried (roster) callsign has been retired/aliased — e.g. a vanity grant replacing it — the roster row is rewritten to the operator's current callsign in the generated PoLo notes file. Affects every consumer of the daemon's notes (CWops, QRQ Crew, SKCC, etc.) and means downstream callsign-enrichment lookups (Carrier Wave server, etc.) match the operator's actual on-air callsign instead of a defunct one.
- Cache schema gained a `current_call` field. Legacy entries without it are treated as expired and re-looked up on next cycle, backfilling the field without losing nicknames.
- `Vec<Member>` is now de-duplicated after remap: when two roster rows resolve to the same current callsign, the one with the lowest numeric member id wins.

## [0.7.0] - 2026-01-05

### Changed
- Replace octocrab with direct reqwest calls for GitHub API (fixes URI format errors)

### Added
- Diagnostic logging for GitHub API calls

## [0.6.3] - 2026-01-05

### Added
- Log token length and validate non-empty before building client

## [0.6.2] - 2026-01-05

### Added
- Diagnostic logging for GitHub API calls to debug URI format errors

## [0.6.1] - 2026-01-05

### Fixed
- Create GitHub client fresh each sync cycle to avoid stale connections after sleep

## [0.6.0] - 2026-01-05

### Added
- Add connectivity diagnostics before each sync cycle (tests DNS/TCP to Google, GitHub)
- Enhanced error logging in fetchers to capture full error chain
- Log connection errors and timeouts explicitly for easier debugging

## [0.5.0] - 2026-01-05

### Added
- Batch all organization updates into a single commit

## [0.4.0] - 2026-01-04

### Added
- SKCC roster support via HTML table scraping

### Fixed
- Collapse nested if statements (clippy)
