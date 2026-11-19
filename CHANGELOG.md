# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.1] — 2026-11-18

### Added

- Light theme. Choose **Theme** in the configuration screen, or set `theme = "light"` in `config.toml`. The dark theme remains the default.

### Fixed

- Colors no longer break in terminals without 24-bit color support, such as Terminal on macOS 15 and earlier. shotctl now uses the closest 256-color palette entries there, and never falls back to the terminal theme's colors. `SHOTCTL_COLOR=truecolor|256` overrides detection.

## [0.1.0] — 2026-11-18

First public release.

### Added

- Interactive terminal UI for importing photos from SD cards and folders into a configured archive
- Detected camera volumes with a `DCIM` folder on macOS and Linux; manual source selection everywhere
- Folder layout presets (month/date, year/month/date, date only) with RAW/JPEG grouping options
- Capture-date and import-date bases, with explicit EXIF fallback reporting
- Preview of destination folders and files, including duplicates, renames, and ignored entries
- Folder selection and one-time custom folder names before import
- BLAKE3 destination deduplication with a persistent hash cache for faster rescans
- Verified copy (temp file → hash check → exclusive publish); originals stay on the source
- Safe cancel during scan/import; configuration saved as TOML under the platform config directory
- Installer script for macOS and Linux (`scripts/install.sh`)
- Homebrew tap: `brew install kontryx/tap/shotctl`

[0.1.1]: https://github.com/kontryx/shotctl/releases/tag/v0.1.1
[0.1.0]: https://github.com/kontryx/shotctl/releases/tag/v0.1.0
