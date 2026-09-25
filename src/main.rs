#![windows_subsystem = "windows"]
mod config;
mod pathutil;
mod sync;
mod synclog;
mod health;
mod gui;
mod update;
mod gamescan;
mod crashlog;

fn main() {
    // v1.6.2: crash forensics — stderr (runtime abort messages) into a file.
    // During the stack-overflow hunt the killer clue was on invisible stderr.
    crashlog::capture_stderr();
    // Crash logger: write to sync.log (no separate file)
    std::panic::set_hook(Box::new(|info| {
        crate::synclog::write(&format!("CRASH: {}", info));
    }));
    if std::env::var("SLINT_BACKEND").is_err() {
        std::env::set_var("SLINT_BACKEND", "software");
    }
    let args: Vec<String> = std::env::args().collect();
    let mut sync_mode = false;
    let mut auto_restore = false;
    let mut link_path = String::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-sync" => sync_mode = true,
            "-autorestore" => auto_restore = true,
            "-link" => { if i + 1 < args.len() { link_path = args[i + 1].clone(); i += 1; } }
            _ => {}
        }
        i += 1;
    }
    if sync_mode {
        // Mutex prevents concurrent sync processes from stacking up
        // C3: per-pair locks now live inside sync_pair_to_cloud/
        // restore_pair_from_cloud (acquire_pair_lock) — every entry point is
        // covered, not just this one.
        config::migrate_to_data_dir();
        config::ensure_versions_setup();
        // INTERVAL-GUARD: a scheduled fire while the previous cycle completed
        // less than one interval ago is a no-op (1-min interval + 4-min backup
        // no longer stacks waiting task instances). The pair lock still guards
        // truly concurrent runs; this guards rapid-fire re-fires.
        let cfg_guard = config::load_config();
        if config::sync_completed_recently(cfg_guard.sync_interval_minutes as i64) {
            return;
        }
        sync::sweep_orphaned_temps(); // L7: headless machines never launch the GUI
        sync::sweep_orphaned_restore_dirs(&config::load_config()); // M-5
        sync::sync_all_pairs();
        return;
    }
    if auto_restore {
        let cfg = config::load_config();
        for j in &cfg.junctions {
            if !j.auto_restore { continue; }
            let p = std::path::Path::new(&j.source_path);
            if !p.exists() || sync::is_dir_empty(&j.source_path) { sync::restore_pair_from_cloud(&j.source_path); }
        }
        return;
    }
    if !link_path.is_empty() {
        // Confirm before syncing (prevents accidental right-click)
        let leaf = std::path::Path::new(&link_path)
            .file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let confirm = rfd::MessageDialog::new()
            .set_title("Sync Folder")
            .set_description(&format!("Sync '{}' to your backup?", leaf))
            .set_buttons(rfd::MessageButtons::YesNo)
            .show() == rfd::MessageDialogResult::Yes;
        if !confirm { return; }

        let mut cfg = config::load_config();
        // L2: normalized dedup — see config::same_path (contracted vs expanded aware)
        cfg.junctions.retain(|j| !config::same_path(&j.source_path, &link_path));
        cfg.junctions.push(config::Junction { source_path: link_path.clone(), auto_restore: true, created: synclog::timestamp(), is_game: false, volume_id: None });
        if !config::save_config(&cfg) {
            rfd::MessageDialog::new()
                .set_title("Error")
                .set_description("Could not save the config (disk full or OneDrive lock?) — the folder was NOT added.")
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
            return;
        }
        let (ok, reason) = sync::sync_pair_to_cloud(&link_path, &cfg.excluded_names, cfg.max_versions, true);
        if ok {
            crate::health::write_status(1, 0, 0, &[]);
            rfd::MessageDialog::new()
                .set_title("Sync Complete")
                .set_description(&format!("'{}' backed up successfully.", leaf))
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
        } else {
            rfd::MessageDialog::new()
                .set_title("Sync Failed")
                .set_description(&format!("Failed to back up '{}'.\n\nError: {}", leaf, reason))
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
        }
        return;
    }
    // Single instance — only for GUI mode (not -sync, -link, -autorestore)
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::Win32::Foundation::GetLastError;
    use std::os::windows::ffi::OsStrExt;
    let mutex_name: Vec<u16> = std::ffi::OsStr::new("LRGEXRestoreSingleInstance")
        .encode_wide().chain(std::iter::once(0)).collect();
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr());
        if GetLastError() == 183 { // ERROR_ALREADY_EXISTS
            return; // Another GUI instance is running — exit silently
        }
        let _ = handle; // keep mutex alive
    }

    gui::run();
}
