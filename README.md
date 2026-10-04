# shotctl

A Rust and Ratatui terminal app for importing camera photos into an organized archive. A compact configuration header, selectable tables, and keyboard shortcuts take inspiration from k9s.

## Run

With Rust 1.88 or newer installed:

```sh
cargo run
```

To install the `shotctl` command on macOS or Linux:

```sh
sh scripts/install.sh
shotctl
```

The installer builds a release binary and installs it into `~/.local/bin`. If that directory is already on your PATH, `shotctl` works immediately from any directory. Otherwise, the installer prints the setup command for your shell. If Cargo is unavailable but a release binary has already been built, it installs that binary.

For **zsh** (the macOS default), add this line to `~/.zshrc`:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

For **bash**, add the same line to `~/.bashrc` on Linux or `~/.bash_profile` on macOS. Open a new terminal afterward, or run the export directly in your current terminal to use `shotctl` immediately.

For **fish**, run this once; the change persists across new terminals:

```fish
fish_add_path "$HOME/.local/bin"
```

You can pass an absolute installation directory to the installer, for example `sh scripts/install.sh /usr/local/bin`, if you have permission to write there.

Alternatively, use Cargo's installer:

```sh
cargo install --path . --locked
```

Cargo installs into `${CARGO_HOME:-$HOME/.cargo}/bin`. Add that directory to PATH if it is missing. For zsh or bash, put `export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"` in the startup file described above; for fish, run `fish_add_path ~/.cargo/bin` (or your custom Cargo bin directory).

You can also choose a source or configuration file explicitly:

```sh
shotctl --source /Volumes/CAMERA
shotctl --source ~/Pictures/Incoming --config ./shotctl.toml
```

Use an interactive terminal, preferably 100 columns × 32 rows or larger. Smaller layouts work down to 66 × 22.

## Your first import

1. Choose an archive directory. An absolute path or `~/` works; saving creates the directory if needed.
2. Pick a folder structure, photo grouping, and date basis. The header and examples update as you edit. Press `Ctrl+S` to save.
3. Select your card. Mounted volumes with a `DCIM` folder are detected on macOS and Linux; press `r` after connecting one. Press `s` to choose any photo directory manually.
4. Press `Enter` to scan. Review exact destination folders, file counts, duplicate skips, renamed conflicts, and metadata fallbacks. Press `f` to inspect individual destination files and their source paths.
5. Press `Enter`, then `Enter` or `y` in the confirmation dialog to import. Follow progress and review the completion summary.

Press `c` to change configuration before another import. Saving or changing the source discards the previous preview so you can review the new destinations.

## Folder presets

For photos dated October 22, 2026:

| Preset | Date folder |
| --- | --- |
| Month / date (default) | `10/2026-10-22` |
| Year / month / date | `2026/10/2026-10-22` |
| Date only | `2026-10-22` |

Each preset supports these grouping options:

| Grouping | Example under `/example-archive` with Month / date |
| --- | --- |
| Separate date folders (default) | `10/2026-10-22-RAW/` and `10/2026-10-22-JPEG/` |
| Type subfolders | `10/2026-10-22/RAW/` and `10/2026-10-22/JPEG/` |
| Together | `10/2026-10-22/` |

The month preset intentionally shares month folders across years; the full date distinguishes the shoots. Choose Year / month / date for separate yearly archives.

**Capture date** uses embedded EXIF `DateTimeOriginal`, then `DateTimeDigitized`, then `DateTime`. Camera dates are used as recorded, without a timezone conversion. If metadata is missing or the container is unsupported, the file's modification date in your computer's local timezone is used. The preview and confirmation explicitly report these fallbacks. JPEG, DNG, and TIFF-based RAW metadata are supported by the EXIF reader; formats such as CR3 and RAF may use the fallback.

**Import date** uses your computer's local date when the scan starts, consistently for the whole import. Rescan to use a new date if the preview stays open past midnight.

## Keys

| Key | Action |
| --- | --- |
| `↑` / `↓`, `j` / `k` | Select a source or scroll a preview |
| `Enter` / `i` | Build a preview; open confirmation from a preview |
| `s` | Enter a source directory |
| `r` | Refresh detected cards; rescan from a preview |
| `c` | Edit configuration |
| `f` / `Tab` | Switch between folder and file previews |
| `PageUp` / `PageDown` | Scroll a preview by ten rows |
| `?` | Open help |
| `Esc` | Go back; close a dialog; cancel an active operation |
| `q` | Quit; cancel when an operation is running |
| `Ctrl+C` | Quit; safely cancel an active operation |

In configuration, use `Tab` / `Shift+Tab` or arrows to select a field, `←` / `→` / `Enter` to cycle options, and `Ctrl+S` to save. Text fields append characters; `Backspace` deletes and `Ctrl+U` clears. On first launch, `Esc` exits without saving.

## Import behavior

- Recursively scans regular photo files. Supports JPEG, common RAW extensions (including ARW, CR2, CR3, DNG, NEF, ORF, RAF, RW2), HEIF, TIFF, and PNG. Other files, videos, sidecars, and symlinks are ignored and counted in the preview.
- Copies photos and preserves their modification times. Originals are kept on the source.
- Compares BLAKE3 hashes at each proposed destination. Identical files at that destination are skipped, including duplicates within the same import. This is destination-based deduplication; changing the date basis or folder settings can create another copy elsewhere.
- Keeps conflicting contents by adding a stable 12-character content hash to the incoming filename, with a counter if necessary. Exact renamed destinations are shown in the preview.
- Writes to a temporary file inside the destination folder, verifies the copied content against the preview's hash, then publishes without overwriting. If a source or destination changes after preview, the affected import fails safely and is reported.
- Scanning and copying run in background threads. Cancelling removes the current temporary file and keeps completed photos. Empty directories may remain. A force kill or power loss can leave a `.shotctl-*.partial` file; it is never treated as a completed photo.
- Rejects source/archive overlap and destination folders that resolve outside the archive. Directory symlinks are not followed during source scanning.

Publishing uses an exclusive rename where the operating system and filesystem support it, with a hard-link fallback. Filesystems that support neither operation report an import error. A failed import can be retried after a new preview; completed photos will be skipped.

## Configuration

Configuration is saved as TOML. `XDG_CONFIG_HOME`, when set, takes precedence. Otherwise the platform's configuration directory is used:

- macOS: `~/Library/Application Support/shotctl/config.toml`
- Linux: `~/.config/shotctl/config.toml`
- Windows: `%APPDATA%\shotctl\config.toml` (manual source selection is available; automatic detection is intended for macOS and Linux).

Use `--config PATH` for a separate profile. Example:

```toml
archive = "/example-archive"
layout = "month_date"
grouping = "separate"
date_basis = "capture"
```

The Rust modules separate configuration, media detection and metadata, import planning and copying, application state, and rendering.

## Development

```sh
cargo test --locked
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
```

Tests use temporary cards and archives, including a TIFF/EXIF fixture, to cover folder presets, metadata fallback, duplicate handling, conflicts, changed files, cancellation, symlinks, configuration persistence, confirmation, and terminal rendering.
