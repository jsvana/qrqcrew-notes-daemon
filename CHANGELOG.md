# Changelog

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
