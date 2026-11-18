# shotctl

A fast, keyboard-driven terminal app for importing photos from your SD card. Preview exactly where every photo will land, skip duplicates, and import with confidence.

Source and issues: [github.com/kontryx/shotctl](https://github.com/kontryx/shotctl)

## Install with Homebrew

On macOS or Linux with [Homebrew](https://brew.sh):

```sh
brew install kontryx/shotctl/shotctl
shotctl
```

Upgrade later with `brew upgrade shotctl`.

## Run from source

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

## How to update Shotctl

If you installed with Homebrew, run `brew upgrade shotctl`. For a source install, after pulling new changes, rebuild and reinstall from the project directory:

```sh
source "$HOME/.cargo/env"
sh scripts/install.sh
```

Then run `shotctl`. A new terminal alone is not enough — the installed binary only updates when you run the installer.

## Your first import

1. Choose an archive directory. Type part of a path and press `Tab` to complete folder names, or press `Enter` / `Ctrl+B` to browse folders visually. An absolute path or `~/` works; saving creates the directory if needed.
2. Pick a folder structure, photo grouping, and date basis. The header and examples update as you edit. Press `Ctrl+S` to save.
3. Select your card. Mounted volumes with a `DCIM` folder are detected on macOS and Linux; press `r` after connecting one. Press `s` to choose any photo directory manually.
4. Press `Enter` to scan. Review exact destination folders, file counts, duplicate skips, renamed conflicts, and metadata fallbacks. Press `f` to inspect individual destination files and their source paths. Folders already in the archive also show an **IMPORTED** date, the day the folder was created (when photos first arrived in it), on terminals at least 78 columns wide and where the filesystem records creation dates; the highlighted folder's date also appears in the detail line below the table.
5. Press `Enter`, then `Enter` or `y` in the confirmation dialog to import. Follow progress and review the completion summary.

After a successful import, press `o` to open the archive folder in your system's file manager and browse the newly created import folders.

In the preview, `Esc` returns from the file view to the folder view. Press `Esc` again from the folder view to return to source selection. If the confirmation dialog is open, `Esc` closes it first.

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
| `f` / `Tab` | Switch between folder and file previews; after a successful import that created one new folder, `f` opens that folder |
| `o` | In the folder preview, open the highlighted folder if it is already in the archive; after a successful import, open the archive folder |
| `p` | After a successful import, open the folder containing the new folders (the parent of a single new folder, or the one shared by several) |
| `Space` | In the folder preview, select or deselect the highlighted new folder |
| `a` | In the folder preview, select all new folders, or deselect them all if every one is already selected |
| `PageUp` / `PageDown` | Scroll a preview by ten rows |
| `?` | Open help |
| `Esc` | Go back; close a dialog; cancel an active operation |
| `q` | Quit; cancel when an operation is running |
| `Ctrl+C` | Quit; safely cancel an active operation |

**Choosing which new folders to create.** By default every folder listed under *New folders* is created. In the folder preview, each new folder has a checkbox: press `Space` to deselect the highlighted folder (`[ ]`) or select it again (`[x]`), or press `a` to deselect all new folders at once and `a` again to select them all. Photos belonging to a deselected folder are not copied, and the totals and confirmation dialog update to match. Existing folders are always used and can't be deselected. Selections apply to the current preview; rescanning selects every new folder again.

**Custom folder.** Press `n` in the folder preview to insert a one-time folder name under the layout's parent folders (for example `2026/10/demo-trip/2026-10-12-RAW/`). The dialog's two scopes, *Choose folders* and *All new folders*, switch with `←` / `→`. Every printable key, including `x` and `Space`, types into the name. In *Choose folders*, use `↑` / `↓` to move, `Tab` to select or deselect a folder, `Enter` to apply, `Ctrl+X` to remove an applied name, and `Esc` to close without applying. The header's example path and the dialog's "Applied now" line show what is currently applied. A custom folder belongs to the current preview and is forgotten when you leave it with `Esc`.

In configuration, use `↑` / `↓` or `Shift+Tab` to select a field, `←` / `→` / `Enter` to cycle options, and `Ctrl+S` to save. `Tab` moves forward from option fields; in the archive path it completes directory names. Text fields append characters; `Backspace` deletes and `Ctrl+U` clears. On first launch, `Esc` exits without saving.

Both the archive path and manual source path support `Tab` completion and `Ctrl+B` to open a folder browser. Completion adds a trailing slash for a unique match and extends a shared prefix when possible. If several matches remain, choose one with `↑` / `↓` and `Tab`, or explore it with `Enter`. Files are excluded; hidden folders are included when you type a dot prefix.

The browser shows the current folder's ancestry and its subfolders. Use `Enter` / `→` to open a subfolder, `←` / `Backspace` to go to its parent, `Space` to use the current folder, or `Tab` to use the highlighted subfolder. `h` jumps home, `/` jumps to the filesystem root, `.` toggles hidden folders, and `PageUp` / `PageDown` scroll. `Esc` returns to the original path without changing it. A path that does not exist yet opens at its nearest existing parent. Choosing a folder fills the path; `Ctrl+S` saves archive settings, and `Enter` accepts a manual source path.

Press `n` in the browser to create a new folder inside the current directory. Enter its name, then press `Enter` to create and open it. `Space` chooses it as the destination, and `Ctrl+S` saves your archive settings. `Esc` cancels the name prompt before creation. Existing files and folders are preserved; duplicate names and permission errors are shown in the prompt.

## Import behavior

- Recursively scans regular photo files. Supports JPEG, common RAW extensions (including ARW, CR2, CR3, DNG, NEF, ORF, RAF, RW2), HEIF, TIFF, and PNG. Other files, videos, sidecars, and symlinks are ignored and counted in the preview.
- Skips known system folders such as macOS `.Trashes`, `.Spotlight-V100`, and `.fseventsd`, and Windows `$RECYCLE.BIN` and `System Volume Information`, before reading their contents. Each skipped folder counts as one ignored entry. Ordinary hidden photo folders are scanned; unreadable photo folders still report an error.
- Copies photos and preserves their modification times. Originals are kept on the source.
- Compares BLAKE3 hashes at each proposed destination. Identical files at that destination are skipped, including duplicates within the same import. This is destination-based deduplication, with one extension: if the destination folder has no such file, the matching folders one level down under a custom name (`2026/10/<name>/2026-10-21-RAW` for `2026/10/2026-10-21-RAW`) are checked too, so photos imported earlier with a custom folder show up as duplicates in the existing folders. Changing the date basis or other folder settings can still create another copy elsewhere.
- Keeps the preview fast. Capture dates in TIFF-based raw files (DNG, ARW, NEF, CR2 and similar) are read from the few bytes of metadata that hold them, not by loading the whole file. Only photos whose name and size already exist at the destination are hashed, several at a time. The hashes are remembered in `hash-cache.tsv` next to the configuration file, so reinserting a card you have imported before does not re-read photos that have not changed. A remembered hash is used only while the file's size and modification time are unchanged; delete `hash-cache.tsv` at any time to force full re-checking.
- Keeps conflicting contents by adding a stable 12-character content hash to the incoming filename, with a counter if necessary. Exact renamed destinations are shown in the preview.
- Writes to a temporary file inside the destination folder, verifies the copied content against the preview's hash, then publishes without overwriting. If a source or destination changes after preview, the affected import fails safely and is reported.
- Scanning and copying run in background threads. Cancelling removes the current temporary file and keeps completed photos. Empty directories may remain. A force kill or power loss can leave a `.shotctl-*.partial` file; it is never treated as a completed photo.
- Rejects source/archive overlap and destination folders that resolve outside the archive. Directory symlinks are not followed during source scanning.

Publishing uses an exclusive rename where the operating system and filesystem support it, with a hard-link fallback. Filesystems that support neither operation report an import error. A failed import can be retried after a new preview; completed photos will be skipped.

## Configuration

Configuration is saved as TOML. `XDG_CONFIG_HOME`, when set, takes precedence. Otherwise the platform's configuration directory is used:

- macOS: `~/Library/Application Support/shotctl/config.toml`
- Linux: `~/.config/shotctl/config.toml`
- Windows: `%APPDATA%\shotctl\config.toml` (build with Cargo; automatic volume detection probes drive letters, but the installer script is macOS/Linux only).

Use `--config PATH` for a separate profile. Example:

```toml
archive = "/example-archive"
layout = "month_date"
grouping = "separate"
date_basis = "capture"
```

The Rust modules separate configuration, media detection and metadata, import planning and copying, application state, and rendering.

## Troubleshooting

- **Terminal too small.** Prefer at least 100×32; layouts work down to 66×22. Enlarge the window if controls look clipped.
- **Card not listed.** Press `r` after mounting, or press `s` and choose the photo directory. Automatic detection looks for a `DCIM` folder on mounted volumes (macOS/Linux) or drive letters (Windows).
- **Unexpected duplicates or missing skips.** Deduplication is destination-based. Changing layout, grouping, or date basis can place the same photo in a new folder. Delete `hash-cache.tsv` next to the config file to force full re-hashing.
- **Leftover `.shotctl-*.partial` files.** A force quit or power loss can leave them; they are never treated as completed photos and can be deleted.
- **Import fails on publish.** Some filesystems lack exclusive rename and hard-link fallbacks; retry after a new preview once the destination is writable.

## License

MIT. See [LICENSE](LICENSE).

## Development

```sh
cargo test --locked
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
```

Tests use temporary cards and archives, including a TIFF/EXIF fixture, to cover folder presets, metadata fallback, duplicate handling, conflicts, changed files, cancellation, symlinks, directory completion and browsing, configuration persistence, confirmation, and terminal rendering.
