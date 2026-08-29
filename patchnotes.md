# Patch Notes — LRGEX Restore

## v1.5.2 — Transparency fix + non-blocking updates

### Fixed the "window goes transparent" bug
- Upgraded the UI renderer (Slint 1.10 → 1.17) — includes upstream fixes for windows rendering see-through after sleep/display changes
- Visually verified at runtime on Windows

### Updates never freeze the app again
- The entire update flow (download → verify → install) now runs on a background thread — a slow or stalled network can no longer freeze the window "(Not Responding)"
- New `update.log`: every update attempt logs each step (fetch, signature, download speed, hash, install) — future "nothing happened" reports are diagnosable in seconds

## v1.5.1 — Saved Games backup fix (known-folder-root bug)

### Fixed
- Protecting a folder that IS a Windows known-folder root (e.g. **Saved Games**, `%KNOWNFOLDER:SavedGames%`) failed with "rename failed" — the backup folder key leaked the token's colon, which Windows forbids in folder names. The key now derives its name from the real folder path; affected backups migrate automatically on the next sync.

### Hardened (root-cause class guard)
- Every derived folder/file name now passes a single sanitizer: illegal characters, trailing dots/spaces, and reserved device names (CON, NUL, COM1…) are neutralized — no future token/unicode leak can ever produce an invalid path
- New test matrix locks the guarantee in (34 tests total)

## v1.5.0 — Security & integrity hardening (3 audit cycles)

### Content-hash change detection
- Same-size file edits (e.g. `AAAAAA` → `BBBBBB`) are now detected — per-file content manifests replace the old size+count check
- Locked files self-heal: a file that was unreadable during one backup is re-learned on the next readable sync
- Restore-invariance: after a format + restore, sync correctly reports "No changes" (content matches, mtimes differ)

### Backup integrity
- zstd frame checksums enabled — corrupted backups fail loudly at restore, never silently restore wrong bytes
- Archive-to-archive overwrite eliminated: two folders with the same name (two games' `Saves`) can never collide — backup identity is now a per-path key
- Migration-safe: old backups (leaf-named or raw folders) migrate automatically on first sync or restore
- Legacy raw-folder migration only deletes the original after the new archive is verified
- Torn copy detection: non-atomic copy fallbacks are size + full-decode verified before being trusted

### Concurrency & safety
- Global mutation lock across all operations (scheduled sync, GUI restore, right-click add, auto-restore) — concurrent restores can never interleave and destroy data
- Missed scheduled runs catch up on wake (StartWhenAvailable) — no more false STALE on machines that sleep
- Short intervals can't stack: a sync that completed within the interval suppresses the next scheduled fire
- Live config re-check each cycle: removing a folder mid-sync no longer resurrects its backup
- Sync source that becomes unreadable aborts instead of archiving an empty set over a good backup

### Update chain (self-update security)
- Manifests are now signed (Ed25519 over version+URL+SHA-256) and verification is FAIL-CLOSED — rollback/downgrade attacks rejected
- Downloaded exe verified against the signed hash; on-disk re-verification after write; updater script re-checks the hash before copying
- Unpredictable temp names, bounded 64 MB download, retry caps

### Path handling
- Path healing works on any system drive (not just C:)
- Unicode-safe token expansion (no panic on exotic characters in configs)
- Case-insensitive home exclusion — backing up a parent of the backup store can't create archive-of-archives

### UI / behavior
- Right-click menu ON by default; setup-complete message on first run; no stray `.lrgex` folder
- Honest dialogs everywhere: interval registration failures, config-save failures, torn archives
- All-skipped sync cycles keep the previous status (no more false "0 folders protected")
- Interval input validated 1–44640 minutes
- Orphaned temp/staging files swept from %TEMP% and junction parents (headless machines included)
- Health checks run off the UI thread — no more 30-second stalls

### Defaults
- Sync interval: 1440 minutes (24 h) · Max versions: 2 · Auto-restore: off

## v1.4.2 — STALE root cause fix

### Scheduled task now catches up on missed runs
- Task uses `StartWhenAvailable=true` — if PC was off/asleep at the scheduled time, the sync runs immediately on wake instead of being skipped entirely
- This was the ROOT CAUSE of false STALE warnings on machines that sleep overnight

### STALE threshold doubled
- Grace period is now 2x the sync interval (48h for daily sync) — tolerates normal scheduling delays without false alarms
- STALE only shows when sync genuinely hasn't run for 2+ full cycles

## v1.4.1 — First-run UX + defaults

### First-run setup polished
- No stray `.lrgex` folder on Desktop/Downloads during setup (check without creating)
- "Setup Complete" message tells user exactly where the app was installed
- Stray `.lrgex` from original location cleaned up after copy to home

### Right-click ON by default
- New installs get the right-click context menu automatically (no Tools toggle needed)
- User can still disable it via Tools → Right-Click Sync

### New defaults
- Sync interval: 1440 minutes (24 hours) — was 120
- Max versions: 2 — was 5

## v1.4.0 — Self-healing + GPL + STALE fix

### Self-healing (move folder anywhere)
- Fixed: app trapped if home folder moved (couldn't open, couldn't Unlink)
- Now self-heals: detects dead old path → re-registers here (registry + task + context menu cleanup)
- Works for both scenarios: fresh exe (no marker) AND moved folder (marker travels)

### STALE fix
- Fixed: false STALE warnings on unchanged folders
- Now checks when the sync LAST RAN (sync-status.json), not backup file age
- No STALE unless the sync itself hasn't run within the interval

### License
- Changed: MIT → GPL-3.0
- Added: TRADEMARK.md (LRGEX name/logo protection)

## v1.3.0 — Progress system + compression engine + stability

### Compression
- Parallel reads (rayon, 24 threads): 139k files 22min → ~5s warm, ~143s cold
- Single-walk design (one pass replaces three)
- Manual tar headers (avoids redundant stat per file)
- ByteReader for large files (>8MB streams, doesn't preload)
- Verified: bsdtar decoded all 139,083 files, zero errors

### Progress bar
- Byte-based: % = bytes done / bytes total (works for any folder)
- Heartbeat: 500ms JSON writer with animated spinner (⠋⠙⠹⠸⠼⠴⠦⠧)
- Live display: spinner + % + MB/s + ETA (never freezes)
- Liveness check: detects dead sync processes (stale heartbeat + dead PID)
- Graceful shutdown: finish() writes terminal snapshot before exit

### Stability
- Fixed: progress frozen at 1% (Drop impl killed heartbeat after first file)
- Fixed: false 'sync stopped unexpectedly' after successful backup
- Fixed: config migrate-loop hang on un-contractable paths (E:\)
- Fixed: folder list not refreshing after right-click add
- Fixed: false STALE warnings on unchanged folders (now checks sync-runtime, not backup-age)

### UI
- 'Protect Folder' / 'Restore Now' buttons
- Natural health labels (All protected, Protecting…, Needs attention)
- Safety summary: green 'N folders protected'
- Tagline: 'Never lose your folders after reinstalling Windows.'

## v1.2.34 — Progress fix + stability
- Byte-based progress: % = bytes done / bytes total (works for any folder)
- Fixed: progress frozen at 1% (Drop impl killed heartbeat after first file)
- Fixed: false "sync stopped unexpectedly" after successful backup
- Fixed: config migrate-loop hang on un-contractable paths (E:\)
- Fixed: folder list not refreshing after right-click add (now watches count)
- Heartbeat: 500ms writer with animated spinner + ETA, never looks frozen
- Parallel reads (rayon): 139k files 22min -> ~5s warm

## v1.2.33 — Fix: Unlink now fully clears home state
- Unlink from Windows now removes the correct marker (`.lrgex/home`) — it was targeting the old pre-migration path (`.lrgex-home`), so the marker survived and re-running the app skipped fresh first-run setup
- First-run setup now writes the marker to the correct location too

## v1.2.32 — Faster compression
- Multi-threaded zstd: compression now uses all CPU cores (was single-threaded) — large folders back up much faster
- Release build re-optimized: fat LTO + codegen-units = 1 (faster runtime, smaller exe)

## v1.2.31 — UI tweaks
- Right-click menu text: "Restore folder (LRGEX)" → "Add to LRGEX Restore" (the action adds the folder for backup, doesn't restore it)
- Title under logo: "LRGEX Restore" → "Restore" (larger hero font)

## v1.2.30 — Renamed to LRGEX Restore
- Project renamed to LRGEX Restore
- New exe name: `LRGEXRestore.exe`
- New registry path, scheduled task name, and right-click menu entry
- New update server path: download.lrgex.com/app/rst/lrgex-restore
- GitHub repo: github.com/LRGEX/Restore
- Existing users: in the OLD app use Tools → Unlink from Windows, then install `LRGEXRestore.exe` fresh into the same home folder (backups are preserved)

## v1.2.25 — Ed25519 Update Signing + Code Documentation
- Ed25519 signed updates: private key on dev PC, public key baked into exe
- Auto-verify signature on every update — blocks tampered downloads
- Fail-fast deploy: aborts if signature is invalid
- Code documentation: all key functions documented for contributors


## v1.2.24 — Cleanup + AM/PM
- Removed all legacy VBS cleanup code
- Timestamps now use AM/PM format
- Skip failed folders in restore (shows which were skipped)
- Right-click sync: create_subkey fix + honest dialog



## v1.2.23 — Restore Progress + Right-Click Fix + File Organization
- Real decompression progress: live byte count + percentage in overlay
- Right-click sync: create_subkey fix (was silently failing)
- Right-click dialog: verifies actual result, honest error message
- Skipped folders shown in restore summary (needs admin)
- All internal files organized in .lrgex/ subfolder (clean root)
- Portable paths with auto-healing (works on any username)
- Static CRT (standalone exe, no VCRUNTIME dependency)
- Software renderer (VM support)
- Consolidated logging (no extra files)




## v1.2.22 — Software Renderer + Static CRT
- Software renderer: works on VMs without GPU (Hyper-V, VirtualBox, RDP)
- Static CRT: standalone exe, zero DLL dependencies





## v1.2.21 — Static CRT + Old Task Cleanup
- Static CRT linking: standalone exe, zero DLL dependencies (fixes VCRUNTIME140.dll error on fresh Windows)
- Auto-cleanup old VBS scheduled tasks on first launch (fixes sync-runner.vbs not found)
- One-time marker gate: cleanup scan runs once, never again
- Schedule fix: MINUTE for <=1439, DAILY for >=1440 (fixes 1442-min interval silent failure)






## v1.2.20 — Portable Paths + Health Check + UX Polish
- Portable config paths: %USERPROFILE%, %LOCALAPPDATA%, %APPDATA%, %PROGRAMFILES% — survives username changes after format
- Known Folder API: %KNOWNFOLDER:SavedGames/ Documents/Desktop/Downloads/Pictures/Music/Videos% — handles relocated folders
- Auto-expand on load, auto-contract on save/export — zero user friction
- 11 unit tests: round-trip, boundary, case-insensitive, UNC, custom drives, forward slashes
- Backup Health Check: shows size, age, source status, STALE flag
- Browse: folder picker + auto-add to sync list + immediate backup
- Remove: asks to delete backup files (with path traversal + empty-name guards)
- Compression: zstd level 1 (2x faster), locked files skip + reported
- Close guard: operation-running flag (backup + restore)
- About button with logo overlay
- Window locked to 560x700 (no maximize)
- Scrollable folder list for 100+ folders
- Deselect: click selected folder to toggle off
- Hover tooltips on action buttons







## v1.2.19 — UX Overhaul + Compression Optimization
- Compression: zstd level 1 (2x faster than level 3)
- Compression: locked files skipped + reported (never silent, never aborts)
- Browse: folder picker starts at current path + auto-adds to sync list + immediate backup
- About button beside Tools (logo + version + info overlay)
- Window locked to 560x700 (no maximize)
- Folder list stretches + scrolls for 100+ folders
- Deselect: click selected folder again to toggle off
- Remove: asks to also delete backup files (with path traversal + empty-name guards)
- Restore: pre-check ALL folders before touching any (no partial restores)
- Restore: atomic rename-swap extraction (destination never half-written)
- Restore-all confirmation dialog
- Close guard: warns if backup or restore is running (instant flag, no timing gap)
- Health bar: no more OK overwrite during active compression
- Startup temp sweep: cleans orphaned temp files from killed processes
- Honest backup messages: file count + size + skipped count








## v1.2.18 — Game Lamp + Skip-List + Real-World Tested
- Game detection lamp per folder (green = game saves detected, cached in config)
- Column headers in folder list (Path | Game | Auto | Versions)
- Skip-list: scanner skips node_modules, .git, target, build, __pycache__, etc.
- Restore Folder button now sets migration marker (was missing — migrations never triggered after manual restore)
- Three-state classification hardened with weighted scoring (Unknown is reachable + tested)
- REFUSED messages hidden from health bar (logged only — non-game numeric dirs like npm year folders)
- Real-world tested: delete → restore → create empty ID → reopen → saves auto-migrated









## v1.2.17 — Post-Launch Migration + Research-Backed Detection
- Migration now fires on GUI launch (background thread, 3s delay, zero UI freeze)
- No more waiting for sync interval after format — open the app, saves migrate instantly
- Post-launch checks orchestrator: extensible pattern for future checks (panic-isolated, documented)
- Migration marker only clears on real migration (stays until saves actually copied)
- Save detection patterns expanded from research on 1,460 games (SaveGameExtractor database)
- Added: saved (116 UE games), saved games (100), savegame (19)
- Removed: wgs (unreachable in current scanner, UWP paths have no numeric IDs)










## v1.2.16 — Save-ID Migration Hardened
- Three-state classification: HasSaveData / FreshInstall / Unknown — refuses migration if ANY folder is ambiguous
- Weighted scoring: save dirs +100, SAVEFILE*/autosave* +50, *.sav/*.save +30, 6+ files +20
- Unknown is now reachable and tested — ambiguous folders abort migration with logged REFUSED message
- Expanded save detection: 9 directory patterns + 6 filename patterns
- Deterministic source ranking: save count > bytes > mtime
- Universal scan (any numeric ID parent, not just users/userdata)
- Post-restore gate: marker file, 7-day window, zero cost on normal syncs











## v1.2.15 — Auto Save-ID Migration
- **Auto-migrates game saves when numeric user-ID folders change after format/reinstall
- Detects old folder (has saves) vs new empty folder (game-created) and copies saves over
- Scoped to users and userdata directories only — never touches unrelated folders
- Smart source selection: most recent saves, tiebreak by file count
- Target safety: only migrates into folders with no save data AND ≤5 files (prevents clobbering)
- Non-destructive: never deletes, never overwrites existing files
- Runs automatically after restore and on every sync cycle
- Sync mutex prevents concurrent sync processes from stacking
- Temp compression in %TEMP% (outside OneDrive) — 10x faster for large folders
- PID-based stale progress detection with 10-min mtime backstop












## v1.2.14 — Sync Reliability Overhaul

### Critical Fixes
- **Sync mutex** — prevents concurrent sync processes from stacking up (was the root cause of stuck "Compressing..." forever)
- **PID-based stale progress detection** — health bar auto-clears if sync process dies or PID gets reused (10-min mtime backstop)
- **Temp file now in %TEMP%** — compresses outside OneDrive, ~10x faster for large folders (Hermes: 10min → ~90s)
- **PID-unique temp filenames** — prevents file corruption if two syncs ever collide
- **Single-pass compression** — eliminated redundant directory walk (was scanning folder twice)

### Restore Improvements
- **Restore now shows real failure state** — red "Restore Failed" with exact reason per folder (was showing fake success)
- **Program Files detection** — failure message tells user to run as admin when target needs elevation
- **Partial restore handling** — shows "Partial: X of N" with failed folder names

### UI Polish
- **Restore overlay enlarged** (460x280) with proper padding + word-wrap for multi-line errors
- **Live compression progress** — shows percentage + file count during compression
- **"Scanning..." feedback** during folder pre-walk (no more frozen look on large folders)












## v1.2.13
- Restore progress overlay (impossible to miss)
- Slint-native input dialog replaces ps_inputbox (no more UI freeze)
- Restore runs on background thread












## v1.2.12
- Right-click sync shows confirmation dialog after completion (success or failure with error)












## v1.2.11
- Preview button: browse snapshot in WinRAR/7-Zip (or extract to temp + Explorer)
- "Backup Folder" renamed to "Backup Now"
- "Uninstall" renamed to "Unlink from Windows..."
- Removed unused code (parse_timestamp, SystemTime imports)












## v1.2.10
- Version limit: keep last N snapshots instead of 90-day retention (default 5)
- Tools menu: Set Max Versions... (user-configurable)












## v1.2.9
- Backup Folder compression runs on background thread (UI stays alive, live progress)
- Set Sync Interval no longer hangs (schtasks on background thread)












## v1.2.7
- Cache-buster fix: download URL appends ?v=version (bypasses Cloudflare cache)












## v1.2.6
- Backup Folder button forces compression (no more silent skip)
- Health bar shows "Compressing [folder]..." during manual compression












## v1.2.5
- Exe renamed to LRGEXRestore.exe everywhere (Cargo [[bin]] name)
- Batch updater retry loop for OneDrive locks
- deploy.ps1 verifies upload size matches local
- Legacy PS task cleanup on startup
- AGENT.md updated (no VBS, no self_replace references)












## v1.2.4
- Replaced self_replace with batch updater (OneDrive-safe: app exits before copy)
- Copy retry loop in updater batch (handles OneDrive lock)
- Old PS task cleanup on startup
- Click folder in list fills source box












## v1.2.3
- Click folder in list fills source box (can re-sync by clicking Backup Folder)
- Equal-width buttons (min-width + stretch)
- Canonical home registry verified












## v1.2.2
- **Root cause fix**: scheduled task uses canonical home path from registry, not current_exe()
- **No VBS runner**: task runs exe directly (windows_subsystem=windows, zero console flash)
- **Single home enforcement**: registry key HKCU\SOFTWARE\LRGEX\Restore\HomePath is the ONE source of truth
- **Migration**: existing installs auto-promote to registry on first launch
- **Stray copy protection**: running from non-home path warns user, refuses to retarget task
- **Uninstall**: clears registry key too












## v1.2.1
- **UI fix**: all 4 bottom buttons equal width (min-width + stretch)












## v1.2.0
- **Compression switched to tar+zstd**: 100x faster than LZMA2, same or better ratio
- **Backup folder structure**: all backups now inside `backup/<folder-name>/` — clean home root
- **No staging**: compresses directly from source (no robocopy temp copy)
- **Rename fix**: copy+delete fallback when OneDrive locks files
- **UI layout fix**: ON/OFF status fixed position, buttons equal width
- **Compression progress**: health lamp shows "Compressing [folder]..." in real-time
- **Health status**: writes immediately on manual sync
- **Right-click confirmation**: Yes/No before syncing
- **Snapshot restore fix**: `.tar.zst` extension detection












## v1.1.3
- **Compression switched to tar+zstd**: 100x faster than LZMA2, same or better ratio. Hermes compresses in seconds, not minutes.
- **No staging**: compresses directly from source (no robocopy temp copy)
- **UI layout fix**: ON/OFF status fixed position, buttons equal width
- **Compression progress**: health lamp shows "Compressing [folder]..." in real-time (3-second polling)
- **Exclusions**: properly skipped during compression
- **Health status**: writes immediately on manual sync, not just scheduled cycles
- **Right-click confirmation**: Yes/No before syncing
- **Health cache**: 3-second timer restores cached health when progress clears












## v1.1.2
- **Compression**: all backups now compressed as .7z (LZMA2) instead of raw files
- **Space savings**: 50 small files compressed from 58K to 3.9K
- **Timestamps preserved**: files restore with exact original modification times (game saves sort correctly)
- **Migration**: existing raw folder backups auto-compress to .7z on first sync
- **Change detection**: file count + total size (catches symmetric add+delete)
- **Versioning**: snapshots are .7z files with hardlink deduplication
- **Corruption handling**: corrupted .7z fails gracefully, logged, no garbage files
- **Uninstall fix**: cleanup runs via detached batch, no UI freeze
- **Self-cleaning scheduled task**: VBS runner with 3-miss grace period
- **Single home enforcement**: blocks second installation












## v1.1.1
- **Uninstall freeze fix**: cleanup runs via detached batch file instead of blocking on UI thread












## v1.1.0
- **Uninstall**: Tools menu -> type "yes" -> removes scheduled task, right-click menu, home marker
- **Single home enforcement**: blocks creating a second home if one already exists
- **VBS runner**: scheduled task runs via wscript.exe (no console window flash), self-cleans after 3 consecutive exe-misses (prevents OneDrive false positives)
- **Non-NTFS fallback**: hardlinks fall back to copy on FAT32/exFAT/network drives
- **Assets folder**: icons moved to assets/
- **Single-home check**: uses health::task_exists() to detect existing installation












## v1.0.8
- **Auto-update fix**: update.rs rewritten with download validation (size check), proper error dialogs at every step, no more silent failures.
- **Assets folder**: icons moved to assets/ for cleaner project structure.
- **Git history purged**: deploy scripts removed from all past commits.
- **.gitattributes**: silenced CRLF/LF warnings.












## v1.0.7
- **Non-NTFS support**: hardlink fallback to copy for FAT32/exFAT/network drives.
- **README cleanup**: stripped to essentials, added Features section.
- **deploy.ps1 read-only**: reads version from Cargo.toml (no auto-bump).












## v1.0.6
- **Auto-update system**: app checks for updates on launch, downloads and swaps the exe via self-replace, relaunches automatically.
- **Deploy pipeline**: deploy.ps1 + deploy.bat for one-click build + upload to server + GitHub Releases.
- **MSVC toolchain**: native Windows toolchain (smaller binaries, no MinGW).
- **Version single-source-of-truth**: gui.rs and update.rs use env!(CARGO_PKG_VERSION). deploy.ps1 reads from Cargo.toml (read-only).
- **README rewritten** as marketing copy.
- **Docs updated** for MSVC, auto-update, and deploy workflow.
- **Clean build**: 0 warnings.












## v1.0.0 — Complete Rust Rewrite
### Architecture
- **Complete rewrite from PowerShell to Rust** (edition 2021, MSVC toolchain).
- **Slint UI** — dark-themed native interface replacing Windows Forms. LRGEX branding, logo, Tools dropdown menu.
- **No more PowerShell dependency** — single self-contained `.exe`, no script execution policy issues.
### Sync Engine
- **Mirror sync (/MIR)** — deletions now propagate to the backup (was copy-only /E in PS version).
- **Hardlink snapshot versioning** — before each /MIR sync, the app creates a full-folder snapshot using NTFS hardlinks. Unchanged files share disk blocks (near-zero space). Only changed files take additional storage. This is how rsync --link-dest and Apple Time Machine work.
- **90-day retention** — old snapshots auto-delete after configurable retention period (default 90 days).
- **Per-file change detection** — snapshots only created when something actually changed (file count, size, OR timestamp). No wasted snapshots on unchanged folders.
- **Version restore** — "Versions" button per folder opens a clickable list of snapshots. Pick a timestamp, restore the entire folder to that point.
### UI
- **Slint dark theme** with LRGEX colors (background #1e1e1e, accent #cb803c).
- **Logo embedded** in the binary via @image-url (compile-time, no runtime file needed).
- **Health lamp** — honest status: RED if task not registered, AMBER if waiting/syncing, GREEN if last sync succeeded. Refreshes live every 30 seconds.
- **Tools dropdown menu** — Health Check, Remove, Export/Import Config, Right-Click Sync, View Sync Log, Set Sync Interval, Manage Exclusions.
- **Backed up folders list** with full paths, auto-restore status, per-folder Versions button.
- **Folder selection** with left accent bar highlight (clean, not full orange fill).
- **No console window** — `#![windows_subsystem = "windows"]` hides the terminal completely.
### Infrastructure
- **Scheduled task self-registration** via `schtasks.exe` (not PowerShell Register-ScheduledTask, which fails with Access Denied on some systems). Task name `LRGEX-Restore-Rust` to avoid conflicts with old PS task.
- **First-run setup** — pick home folder, app copies itself there, creates `.lrgex-home` marker, relaunches.
- **Config backward-compatible** — reads PowerShell's PascalCase JSON format (`Junctions`, `SourcePath`, etc.) with BOM stripping.
- **Right-click context menu** — proper registry structure with display name, icon, and command.
- **Log per-installation** — sync.log lives in the home folder, not shared with PowerShell version's LOCALAPPDATA log.
- **Local timezone** — all timestamps use the system's local timezone (chrono crate).
### Versioning Details
- Snapshot folder structure: `_versions/<folder_name>/<YYYYMMDD_HHMMSS>/` (hidden + system attribute).
- Deleted files: hardlinked to snapshot (survives /MIR purge).
- Modified files: old version copied to snapshot (survives /MIR overwrite).
- Unchanged files: hardlinked to snapshot (near-zero space).
- `_versions/` excluded from robocopy via `/XD` to prevent purging.
- One-time migration: old `_trash/` auto-renamed to `_versions/`.
---












## v0.7.0 (PowerShell — superseded by v1.0.0)
- Default sync interval = 120 min (2 hours).
- Decoupled right-click from the sync task.
- Self-healing sync task.
- Synced-folders list in the main UI.
- Absence-driven auto-restore.
- Exclude feature (Manage Exclusions).
- VBS launcher for invisible background sync.
- Custom LRGEX icon in right-click context menu.












## v0.6.x — v0.5.0 (PowerShell — archived)
See git history for details. These versions used Windows Forms + PowerShell + copy-only (/E) sync without versioning.

## v1.2.26 — Key Rotation + Security Hardening
- Rotated Ed25519 signing keypair (old key path was exposed)
- New public key baked into exe
- Old exe users must manually update once
