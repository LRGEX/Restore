<div align="center">

<img src="assets/logo.png" alt="LRGEX Logo" width="220">

# LRGEX Restore
Your Windows folders. Remembered.

**Version 1.5.3**

**Automatic folder backup, versioning, and restore after a Windows reinstall.**

**Portable • Open Source • GPL-3.0 Licensed**




</div>

<table align="center">
<tr>
<td align="center">
<img width="1377" height="774" alt="Recording 2026-08-04 064951 (2)" src="https://github.com/user-attachments/assets/c12d3e0b-6166-4692-adb4-5af26ed3025c" />
</td>
</tr>
</table>

---

## Ever formatted your PC?

You know the feeling.

Windows is fresh. Your apps are reinstalled. Then you launch your favorite game...

**Your saves are gone, or important folders is gone!**

Your application settings? **Gone.**

Hours of progress and years of customization disappear because Windows doesn't protect those folders.

**LRGEX Restore remembers them, keeps them synchronized, and restores them to their exact original locations after you reinstall Windows.**

---

## How it works

1. **Choose a folder** — game saves, app settings, projects, or anything important.
2. **LRGEX Restore watches it** — changes are compressed and synchronized automatically.
3. **Reinstall Windows** — without worrying about lost files.
4. **Restore with one click** — every folder returns to its original location automatically.

**Set it once. Forget about it. Your files are always there when you need them.**

---

## Why LRGEX Restore?

- **One-click protection** — right-click any folder and choose **"Add to LRGEX Restore"**.
- **Works with any cloud** — OneDrive, Google Drive, Dropbox, Mega, iCloud, NAS, Syncthing, or even a local drive.
- **Automatic synchronization** — changes are backed up in the background.
- **Built-in version history** — restore previous snapshots whenever you need them.
- **Automatic restore** — after reinstalling Windows, missing folders are restored to their original paths.
- **Portable** — a single 17 MB executable. No installer. No dependencies.

---

## Features

- **Automatic backup** — new and changed files back up to your cloud automatically
- **Snapshot versioning** — every change creates a snapshot. Roll back any folder to any point in the last 90 days
- **Auto-restore** — after a format, missing folders are restored automatically
- **Right-click integration** — right-click any folder in Explorer to protect it
- **Configurable sync interval** — 1 minute or more, your choice
- **Exclusions** — skip app-locked subfolders that cause false errors
- **Export/Import** — move your folder list to a new PC
- **Health lamp** — live status: green (safe), amber (syncing), red (problem)
- **Auto-update** — the app checks for new versions and updates itself

### What happens when you delete files?

- **Delete one file**: it leaves the backup. The old version lives in versioning for 90 days. Does NOT come back on restore.
- **Delete the entire folder**: if auto-restore is ON, auto-restore brings it all back.

---

## Installation

1. Download `LRGEXRestore.exe`
2. Run it — pick a **home folder** inside your cloud service (OneDrive, Google Drive, etc.) so your files survive a format. Local folder works too, but won't survive a format.
3. Open the app from the home folder
4. Right-click any folder you want to protect — select **"Add to LRGEX Restore"** (menu is enabled automatically; toggle via Tools → Right-Click Sync)

Your folders are now backed up continuously.

---

## Restore after a format

1. Reinstall your cloud service and let it download
2. Open `LRGEXRestore.exe` from your home folder
3. Click **"Restore Saved"** — or let auto-restore handle it automatically

Every folder goes back to its exact original path.

---

## Requirements

- Windows 10/11
- A cloud service recommended (survives a format). Local folder works but won't survive a format.

## Windows Defender false positive

Windows Defender may flag LRGEX Restore as a false positive (`Trojan:Win32/Bearfoos.B!ml` / `Behavior:Win32/Persistence.A!ml`). **This is a confirmed false positive.** The exe is [verified clean on VirusTotal](https://www.virustotal.com/gui/file-analysis/Y2U4ZjhhYmM3ZDc0NDRkNzk5MDZhMjdhYjAxYzNjNzk6MTc4NjczMTY3Ng==) — 0 detections across 70+ engines. Defender's machine learning flags the app because it:

- Copies itself to your chosen home folder (self-copy)
- Creates a scheduled task for automatic syncing (persistence)
- Registers a right-click context menu (registry persistence)

These are the app's core features, not malware behavior. The source code is fully open under GPL-3.0.

**To fix:** add the home folder to Defender exclusions:

1. **Settings → Privacy & Security → Windows Security → Virus & threat protection**
2. **Manage settings → Add or remove exclusions → Add an exclusion → Folder**
3. Select your LRGEX Restore home folder

---

## License

GNU General Public License v3. See [LICENSE](LICENSE) and [TRADEMARK.md](TRADEMARK.md).
