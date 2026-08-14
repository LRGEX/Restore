#![allow(unused_imports)]
use std::os::windows::process::CommandExt;
slint::slint! {
    import { Button, LineEdit, VerticalBox, HorizontalBox, ScrollView } from "std-widgets.slint";

    export struct FolderEntry {
        path: string,
        name: string,
        auto_restore: string,
        is_game: bool,
    }

    export struct VersionEntry {
        display: string,
    }

    component MenuItem inherits Rectangle {
        in property <string> label;
        callback clicked();
        height: 30px;
        background: ta.has-hover ? #282828 : transparent;
        Text {
            text: label;
            color: #f0f0f0;
            vertical-alignment: center;
        }
        ta := TouchArea {
            clicked => { root.clicked(); }
        }
    }

    export component App inherits Window {
        title: "LRGEX Restore " + root.app-version;
        icon: @image-url("../assets/app-icon.png");
        preferred-width: 560px;
        preferred-height: 700px;
        min-width: 560px;
        max-width: 560px;
        min-height: 700px;
        max-height: 700px;
        background: #1e1e1e;

        in-out property <string> health-text: " Checking...";
        in-out property <color> health-color: #4caf50;
        in-out property <string> status-text: "";
        in-out property <string> source-text: "";
        in-out property <[FolderEntry]> folders: [];
        in-out property <string> rc-label: "Right-Click Sync: OFF";
        in-out property <bool> menu-open: false;
        in-out property <int> selected-index: -1;
        in-out property <bool> versions-visible: false;
        in-out property <[VersionEntry]> versions-list: [];
        in-out property <int> selected-version: -1;
        in-out property <string> app-version: "";
        in-out property <string> versions-title: "Versions";
        in-out property <bool> input-visible: false;
        in-out property <bool> restore-visible: false;
        in-out property <bool> about-visible: false;
        in-out property <string> restore-msg: "";
        in-out property <bool> restore-done: false;
        in-out property <bool> operation-running: false;
        in-out property <bool> restore-failed: false;
        callback restore-ok();
        in-out property <string> input-title: "";
        in-out property <string> input-prompt: "";
        in-out property <string> input-value: "";
        in-out property <int> input-mode: 0;  // 0=interval, 1=maxversions, 2=exclusions, 3=unlink

        callback browse-clicked();
        callback link-clicked();
        callback restore-clicked();
        callback toggle-clicked();
        callback remove-clicked();
        callback log-clicked();
        callback rightclick-clicked();
        callback health-clicked();
        callback export-clicked();
        callback import-clicked();
        callback interval-clicked();
        callback maxversions-clicked();
        callback exclusions-clicked();
        callback uninstall-clicked();
        callback versions-clicked(int);
        callback restore-version();
        callback preview-version();
        callback close-versions();
        callback input-ok();
        callback input-cancel();

        VerticalBox {
            spacing: 12px;

            // Tools menu bar
            HorizontalLayout {
                height: 32px;
                spacing: 0px;
                Rectangle {
                    width: 72px;
                    height: 30px;
                    background: root.menu-open ? #cb803c : #2d2d2d;
                    Text {
                        text: "Tools";
                        color: root.menu-open ? white : #f0f0f0;
                        horizontal-alignment: center;
                        vertical-alignment: center;
                        font-weight: 700;
                    }
                    TouchArea { clicked => { root.menu-open = !root.menu-open; } }
                }
                // About button
                Rectangle {
                    width: 60px;
                    height: 30px;
                    background: about-ta.has-hover ? #2d2d2d : transparent;
                    border-radius: 4px;
                    Text {
                        text: "About";
                        color: about-ta.has-hover ? #cb803c : #f0f0f0;
                        horizontal-alignment: center;
                        vertical-alignment: center;
                        font-weight: 700;
                    }
                    about-ta := TouchArea { clicked => { root.about-visible = true; } }
                }
            }

            // Logo (centered, smaller)
            HorizontalLayout {
                alignment: center;
                Image { source: @image-url("../assets/logo.png"); width: 240px; height: 50px; image-fit: contain; }
            }
            HorizontalLayout {
                alignment: center;
                Text { text: "Restore"; font-size: 32px; font-weight: 900; color: #b3b3b3; letter-spacing: 1px; }
            }
            HorizontalLayout {
                alignment: center;
                Text { text: "v" + root.app-version; font-size: 11px; color: #888; }
            }
            HorizontalLayout {
                alignment: center;
                Text { text: "Never lose your folders after reinstalling Windows."; font-size: 11px; color: #777; }
            }

            // Health lamp
            Rectangle {
                horizontal-stretch: 1;
                height: 28px;
                background: root.health-color;
                Text {
                    text: root.health-text;
                    width: parent.width;
                    color: white;
                    font-weight: 700;
                    font-size: 11px;
                    vertical-alignment: center;
                    horizontal-alignment: center;
                    overflow: elide;
                }
            }

            // Source input
            HorizontalBox {
                spacing: 6px;
                LineEdit {
                    placeholder-text: "Folder to protect...";
                    text <=> root.source-text;
                    width: 440px;
                }
                Button { text: "Browse"; clicked => { root.browse-clicked(); } }
            }

            // Backed up folders list (right under source input, shows full paths)
            Text { text: root.folders.length > 0 ? "✓ " + root.folders.length + " folders protected" : "No folders protected yet"; color: root.folders.length > 0 ? #4caf50 : #888; font-size: 12px; font-weight: 700; }

            // Column headers
            HorizontalLayout {
                height: 18px;
                spacing: 0px;
                Text { text: "Folder"; color: #777; font-size: 9px; vertical-alignment: center; horizontal-stretch: 1; }
                Text { text: "Game"; color: #777; font-size: 9px; vertical-alignment: center; horizontal-alignment: center; width: 34px; }
                Text { text: "Auto"; color: #777; font-size: 9px; vertical-alignment: center; horizontal-alignment: center; width: 30px; }
                Text { text: "Versions"; color: #777; font-size: 9px; vertical-alignment: center; horizontal-alignment: center; width: 68px; }
            }

            ScrollView {
                vertical-stretch: 1;
                min-height: 130px;
                VerticalBox {
                    for entry[i] in root.folders : Rectangle {
                        height: 38px;
                        background: i == root.selected-index ? #2d2d2d : #252525;
                        // Left accent bar (orange when selected, invisible otherwise)
                        Rectangle {
                            x: 0px;
                            width: 3px;
                            height: parent.height;
                            background: i == root.selected-index ? #cb803c : transparent;
                        }
                        TouchArea { clicked => { root.selected-index = root.selected-index == i ? -1 : i; root.source-text = root.selected-index == i ? entry.path : ""; } }
                        HorizontalLayout {
                            Rectangle { width: 10px; }
                            VerticalLayout {
                                horizontal-stretch: 1;
                                alignment: center;
                                spacing: 1px;
                                Text {
                                    text: entry.name;
                                    color: i == root.selected-index ? #cb803c : #f0f0f0;
                                    font-size: 13px;
                                    overflow: elide;
                                }
                                Text {
                                    text: entry.path;
                                    color: #5a5a5a;
                                    font-size: 9px;
                                    overflow: elide;
                                }
                            }
                            // Game lamp: green if game saves detected, dark otherwise
                            Rectangle {
                                width: 34px;
                                height: parent.height;
                                Rectangle {
                                    x: (parent.width - self.width) / 2;
                                    y: (parent.height - self.height) / 2;
                                    width: 10px; height: 10px;
                                    border-radius: 5px;
                                    background: entry.is_game ? #4caf50 : #444;
                                }
                            }
                            Text {
                                text: entry.auto_restore;
                                color: entry.auto_restore == "ON" ? #4caf50 : #888;
                                vertical-alignment: center;
                                width: 30px;
                                horizontal-alignment: center;
                            }
                            Rectangle {
                                width: 68px;
                                height: 20px;
                                y: 4px;
                                background: vbtn.has-hover ? #3a3a3a : #2d2d2d;
                                border-radius: 3px;
                                Text { text: "Versions"; color: #cb803c; font-size: 10px; horizontal-alignment: center; vertical-alignment: center; }
                                vbtn := TouchArea { clicked => { root.versions-clicked(i); } }
                            }
                        }
                    }
                }
            }

            // Action buttons (at the bottom — all equal width)
            HorizontalBox {
                spacing: 12px;
                Rectangle {
                    horizontal-stretch: 1; min-width: 250px; height: 32px;
                    background: bk-hover.has-hover ? #3a3a3a : #2d2d2d;
                    border-radius: 4px;
                    Text { text: "Protect Folder"; color: #cb803c; horizontal-alignment: center; vertical-alignment: center; font-weight: 700; }
                    bk-hover := TouchArea { clicked => { root.link-clicked(); } }
                    // Tooltip on hover
                    if bk-hover.has-hover : Rectangle {
                        y: -28px; x: 0px; width: 220px; height: 22px;
                        background: #111;
                        border-radius: 3px; border-width: 1px; border-color: #555;
                        Text { text: "Protect this folder now"; color: #ccc; font-size: 10px; horizontal-alignment: center; vertical-alignment: center; }
                    }
                }
                Rectangle {
                    horizontal-stretch: 1; min-width: 250px; height: 32px;
                    background: rs-hover.has-hover ? #3a3a3a : #2d2d2d;
                    border-radius: 4px;
                    Text { text: "Restore Now"; color: #cb803c; horizontal-alignment: center; vertical-alignment: center; font-weight: 700; }
                    rs-hover := TouchArea { clicked => { root.restore-clicked(); } }
                    if rs-hover.has-hover : Rectangle {
                        y: -28px; x: 0px; width: 220px; height: 22px;
                        background: #111; border-radius: 3px; border-width: 1px; border-color: #555;
                        Text { text: "Restore this folder from your backup"; color: #ccc; font-size: 10px; horizontal-alignment: center; vertical-alignment: center; }
                    }
                }
            }
            HorizontalBox {
                spacing: 12px;
                Rectangle {
                    horizontal-stretch: 1; min-width: 250px; height: 32px;
                    background: tg-hover.has-hover ? #3a3a3a : #2d2d2d;
                    border-radius: 4px;
                    Text { text: "Toggle Auto-Restore"; color: #f0f0f0; horizontal-alignment: center; vertical-alignment: center; }
                    tg-hover := TouchArea { clicked => { root.toggle-clicked(); } }
                    if tg-hover.has-hover : Rectangle {
                        y: -28px; x: 0px; width: 240px; height: 22px;
                        background: #111; border-radius: 3px; border-width: 1px; border-color: #555;
                        Text { text: "Enable/disable automatic restore after format"; color: #ccc; font-size: 10px; horizontal-alignment: center; vertical-alignment: center; }
                    }
                }
                Rectangle {
                    horizontal-stretch: 1; min-width: 250px; height: 32px;
                    background: rm-hover.has-hover ? #3a3a3a : #2d2d2d;
                    border-radius: 4px;
                    Text { text: "Stop Protecting"; color: #f0f0f0; horizontal-alignment: center; vertical-alignment: center; }
                    rm-hover := TouchArea { clicked => { root.remove-clicked(); } }
                    if rm-hover.has-hover : Rectangle {
                        y: -28px; x: 0px; width: 200px; height: 22px;
                        background: #111; border-radius: 3px; border-width: 1px; border-color: #555;
                        Text { text: "Stop protecting this folder — your files are NOT deleted"; color: #ccc; font-size: 10px; horizontal-alignment: center; vertical-alignment: center; }
                    }
                }
            }

            Text { text: root.status-text; color: #cb803c; font-size: 12px; wrap: word-wrap; horizontal-stretch: 1; }
        }

        // Tools dropdown overlay
        if root.menu-open : Rectangle {
            x: 0px;
            y: 0px;
            width: root.width;
            height: root.height;
            background: transparent;
            TouchArea { clicked => { root.menu-open = false; } }

            Rectangle {
                x: 8px;
                y: 40px;
                width: 240px;
                height: 308px;
                background: #191919;
                border-radius: 4px;
                border-width: 1px;
                border-color: #3a3a3a;

                VerticalLayout {
                    MenuItem {
                        label: "Junction Health Check";
                        clicked => { root.health-clicked(); root.menu-open = false; }
                    }
                    Rectangle { height: 1px; background: #3a3a3a; }
                    MenuItem {
                        label: "Export Configuration";
                        clicked => { root.export-clicked(); root.menu-open = false; }
                    }
                    MenuItem {
                        label: "Import Configuration";
                        clicked => { root.import-clicked(); root.menu-open = false; }
                    }
                    Rectangle { height: 1px; background: #3a3a3a; }
                    MenuItem {
                        label: root.rc-label;
                        clicked => { root.rightclick-clicked(); root.menu-open = false; }
                    }
                    Rectangle { height: 1px; background: #3a3a3a; }
                    MenuItem {
                        label: "View Sync Log";
                        clicked => { root.log-clicked(); root.menu-open = false; }
                    }
                    MenuItem {
                        label: "Set Sync Interval...";
                        clicked => { root.interval-clicked(); root.menu-open = false; }
                    }
                    MenuItem {
                        label: "Set Max Versions...";
                        clicked => { root.maxversions-clicked(); root.menu-open = false; }
                    }
                    MenuItem {
                        label: "Manage Exclusions...";
                        clicked => { root.exclusions-clicked(); root.menu-open = false; }
                    }
                    Rectangle { height: 1px; background: #3a3a3a; }
                    MenuItem {
                        label: "Unlink from Windows...";
                        clicked => { root.uninstall-clicked(); root.menu-open = false; }
                    }
                }
            }
        }

        // Versions overlay panel
        if root.versions-visible : Rectangle {
            x: 0px;
            y: 0px;
            width: root.width;
            height: root.height;
            background: rgba(0, 0, 0, 0.75);

            Rectangle {
                x: (root.width - self.width) / 2;
                y: (root.height - self.height) / 2;
                width: 360px;
                height: 420px;
                background: #1e1e1e;
                border-radius: 8px;
                border-width: 1px;
                border-color: #3a3a3a;

                VerticalLayout {
                    padding: 16px;
                    spacing: 12px;

                    Text {
                        text: root.versions-title;
                        font-size: 16px;
                        font-weight: 800;
                        color: #b3b3b3;
                        horizontal-alignment: center;
                    }

                    Rectangle { height: 1px; background: #3a3a3a; }

                    ScrollView {
                        vertical-stretch: 1;
                        VerticalLayout {
                            spacing: 2px;
                            for entry[i] in root.versions-list : Rectangle {
                                height: 30px;
                                background: i == root.selected-version ? #cb803c : (vta.has-hover ? #2d2d2d : transparent);
                                border-radius: 3px;
                                vta := TouchArea { clicked => { root.selected-version = i; } }
                                Text {
                                    text: entry.display;
                                    color: i == root.selected-version ? white : #f0f0f0;
                                    vertical-alignment: center;
                                }
                            }
                        }
                    }

                    HorizontalLayout {
                        spacing: 12px;
                        alignment: center;
                        Rectangle {
                            width: 100px; height: 30px;
                            background: pbtn.has-hover ? #3a3a3a : #2d2d2d;
                            border-radius: 4px;
                            Text { text: "Preview"; color: #cb803c; horizontal-alignment: center; vertical-alignment: center; font-weight: 700; }
                            pbtn := TouchArea { clicked => { root.preview-version(); } }
                        }
                        Rectangle {
                            width: 100px; height: 30px;
                            background: rbtn.has-hover ? #d89554 : #cb803c;
                            border-radius: 4px;
                            Text { text: "Restore"; color: white; horizontal-alignment: center; vertical-alignment: center; font-weight: 700; }
                            rbtn := TouchArea { clicked => { root.restore-version(); } }
                        }
                        Rectangle {
                            width: 100px; height: 30px;
                            background: cbtn.has-hover ? #3a3a3a : #2d2d2d;
                            border-radius: 4px;
                            Text { text: "Close"; color: #f0f0f0; horizontal-alignment: center; vertical-alignment: center; }
                            cbtn := TouchArea { clicked => { root.close-versions(); } }
                        }
                    }
                }
            }
        }

        // Input dialog overlay
        if root.input-visible : Rectangle {
            x: 0px; y: 0px;
            width: root.width; height: root.height;
            background: rgba(0, 0, 0, 0.75);
            TouchArea { clicked => { } }

            Rectangle {
                x: (root.width - self.width) / 2;
                y: (root.height - self.height) / 2;
                width: 380px;
                height: 180px;
                background: #1e1e1e;
                border-radius: 8px;
                border-width: 1px;
                border-color: #3a3a3a;

                VerticalLayout {
                    padding: 16px;
                    spacing: 12px;

                    Text {
                        text: root.input-title;
                        font-size: 16px;
                        font-weight: 800;
                        color: #b3b3b3;
                    }
                    Text {
                        text: root.input-prompt;
                        font-size: 12px;
                        color: #aaa;
                        wrap: word-wrap;
                    }
                    LineEdit {
                        text <=> root.input-value;
                        placeholder-text: "Enter value...";
                    }

                    HorizontalLayout {
                        spacing: 12px;
                        alignment: end;
                        Rectangle {
                            width: 80px; height: 30px;
                            background: okbtn.has-hover ? #d89554 : #cb803c;
                            border-radius: 4px;
                            Text { text: "OK"; color: white; horizontal-alignment: center; vertical-alignment: center; font-weight: 700; }
                            okbtn := TouchArea { clicked => { root.input-ok(); } }
                        }
                        Rectangle {
                            width: 80px; height: 30px;
                            background: cnclbtn.has-hover ? #3a3a3a : #2d2d2d;
                            border-radius: 4px;
                            Text { text: "Cancel"; color: #f0f0f0; horizontal-alignment: center; vertical-alignment: center; }
                            cnclbtn := TouchArea { clicked => { root.input-cancel(); } }
                        }
                    }
                }
            }
        }

        // Restore overlay — impossible to miss
        if root.restore-visible : Rectangle {
            x: 0px; y: 0px;
            width: root.width; height: root.height;
            background: rgba(0, 0, 0, 0.85);
            TouchArea { clicked => { } }

            Rectangle {
                x: (root.width - self.width) / 2;
                y: (root.height - self.height) / 2;
                width: 460px;
                height: 280px;
                background: #1e1e1e;
                border-radius: 8px;
                border-width: 1px;
                border-color: #cb803c;

                VerticalLayout {
                    padding: 24px;
                    spacing: 12px;

                    Text {
                        text: root.restore-done ? (root.restore-failed ? "Restore Failed" : "Restore Complete") : "Restoring...";
                        font-size: 18px;
                        font-weight: 800;
                        color: root.restore-done ? (root.restore-failed ? #f44336 : #4caf50) : #cb803c;
                        horizontal-alignment: center;
                        horizontal-stretch: 1;
                    }
                    Text {
                        text: root.restore-msg;
                        font-size: 12px;
                        color: #f0f0f0;
                        horizontal-alignment: center;
                        horizontal-stretch: 1;
                        wrap: word-wrap;
                    }
                    if root.restore-done : HorizontalLayout {
                        alignment: center;
                        Rectangle {
                            width: 80px; height: 28px;
                            background: rokbtn.has-hover ? #d89554 : #cb803c;
                            border-radius: 4px;
                            Text { text: "OK"; color: white; horizontal-alignment: center; vertical-alignment: center; font-weight: 700; }
                            rokbtn := TouchArea { clicked => { root.restore-ok(); } }
                        }
                    }
                }
            }

        }

        // About overlay — sibling of restore overlay (not nested inside)
        if root.about-visible : Rectangle {
            x: 0px; y: 0px;
            width: root.width; height: root.height;
            background: rgba(0, 0, 0, 0.85);
            TouchArea { clicked => { } }

            Rectangle {
                x: (root.width - self.width) / 2;
                y: (root.height - self.height) / 2;
                width: 360px;
                height: 380px;
                background: #1e1e1e;
                border-radius: 8px;
                border-width: 1px;
                border-color: #cb803c;

                VerticalLayout {
                    alignment: center;
                    spacing: 14px;

                    HorizontalLayout {
                        alignment: center;
                        Image {
                            source: @image-url("../assets/logo.png");
                            width: 200px; height: 48px;
                            image-fit: contain;
                        }
                    }

                    Text {
                        text: "LRGEX Restore";
                        font-size: 20px; font-weight: 900; color: #b3b3b3;
                        horizontal-alignment: center;
                    }

                    Text {
                        text: "v" + root.app-version;
                        font-size: 12px; color: #888;
                        horizontal-alignment: center;
                    }

                    Text {
                        text: "Cloud-agnostic folder backup\nwith snapshot versioning\nand auto-restore.";
                        font-size: 11px; color: #aaa;
                        horizontal-alignment: center;
                    }

                    Rectangle { height: 1px; background: #333; }

                    Text {
                        text: "\u{a9} LRGEX. All rights reserved.";
                        font-size: 10px; color: #666;
                        horizontal-alignment: center;
                    }

                    HorizontalLayout {
                        alignment: center;
                        Rectangle {
                            width: 80px; height: 28px;
                            background: aboutok.has-hover ? #d89554 : #cb803c;
                            border-radius: 4px;
                            Text { text: "OK"; color: white; horizontal-alignment: center; vertical-alignment: center; font-weight: 700; }
                            aboutok := TouchArea { clicked => { root.about-visible = false; } }
                        }
                    }
                }
            }
        }
    }
}
use slint::VecModel;

fn fmt_dur(secs: f64) -> String {
    let s = secs as u64;
    if s >= 60 { format!("{}m {}s", s / 60, s % 60) } else { format!("{}s", s) }
}
use crate::{config, sync, health, synclog};

pub fn run() {
    // Startup sweep: clean up orphaned temp files from killed compressions.
    config::migrate_to_data_dir();
    sync::sweep_orphaned_temps();
    sync::sweep_orphaned_restore_dirs(&config::load_config()); // M-5: killed-restore staging dirs

    // First-run: relocate to home folder if needed
    if !config::is_home() {
        // Check if already installed elsewhere
        if health::task_exists() || config::canonical_home().is_some() {
            let existing = config::canonical_home();
            // If the old home's exe still exists → legit other install, refuse.
            // If it's GONE (folder moved/deleted) → auto re-home here (no trap).
            let old_exe = existing.as_ref().map(|h| h.join("LRGEXRestore.exe"));
            let old_still_there = old_exe.as_ref().map(|e| e.exists()).unwrap_or(false);
            if old_still_there {
                let existing_str = existing
                    .map(|h| h.to_string_lossy().to_string())
                    .unwrap_or_else(|| "an unknown location".to_string());
                rfd::MessageDialog::new()
                    .set_title("Already Installed")
                    .set_description(&format!("LRGEX Restore is already installed at:\n{}\n\nOpen it from there.\n\nTo move: Tools -> Unlink from Windows first, then run this exe again.", existing_str))
                    .set_buttons(rfd::MessageButtons::Ok)
                    .show();
                return;
            } else {
                // Old home is gone — clean up ALL stale references (full cascade):
                use std::os::windows::process::CommandExt;
                // 1. Registry canonical home
                config::clear_canonical_home();
                // 2. Scheduled task (points to dead exe path)
                let _ = std::process::Command::new("schtasks.exe")
                    .args(["/Delete", "/TN", "LRGEX-Restore-Rust", "/F"])
                    .creation_flags(0x08000000u32)
                    .output();
                // 3. Right-click context menu (points to dead exe path)
                let _ = std::process::Command::new("reg.exe")
                    .args(["delete", r"HKCU\Software\Classes\Directory\shell\LRGEXRestore", "/f"])
                    .creation_flags(0x08000000u32)
                    .output();
                // Fall through to setup_home() → re-homes here.
                // New exe on next launch registers fresh task (register_sync_task at line ~708).
            }
        }
        setup_home();
        return;
    }

    // Migration: if canonical home not in registry yet, promote current home
    if config::canonical_home().is_none() {
        config::set_canonical_home(&config::script_dir());
    }

    // Verify: this exe IS the canonical home exe
    if let Some(canonical) = config::canonical_home() {
        if config::script_dir() != canonical {
            // Self-heal: if the old canonical home is GONE (folder moved),
            // re-register here instead of trapping the user.
            let old_exe = canonical.join("LRGEXRestore.exe");
            if !old_exe.exists() {
                config::set_canonical_home(&config::script_dir());
                use std::os::windows::process::CommandExt;
                let _ = std::process::Command::new("schtasks.exe")
                    .args(["/Delete", "/TN", "LRGEX-Restore-Rust", "/F"])
                    .creation_flags(0x08000000u32)
                    .output();
                let _ = std::process::Command::new("reg.exe")
                    .args(["delete", r"HKCU\Software\Classes\Directory\shell\LRGEXRestore", "/f"])
                    .creation_flags(0x08000000u32)
                    .output();
            } else {
            rfd::MessageDialog::new()
                .set_title("Wrong Copy")
                .set_description(&format!("This is not the installed copy.\n\nThe real installation is at:\n{}", canonical.display()))
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
            return;
            }
        }
    }

    // Self-heal: register task using canonical home (never current_exe)
    let cfg0 = config::load_config();
    sync::register_sync_task(cfg0.sync_interval_minutes);
    config::ensure_versions_setup();

    let app = App::new().unwrap();
    app.set_app_version(env!("CARGO_PKG_VERSION").into());

    // Initial state
    refresh_folders(&app);
    let h = health::get_health();
    app.set_health_text(format!(" {} ", h.label).into());
    app.set_health_color(match h.status.as_str() {
        "GREEN" => slint::Color::from_rgb_u8(76, 175, 80),
        "AMBER" => slint::Color::from_rgb_u8(200, 140, 0),
        _ => slint::Color::from_rgb_u8(200, 30, 30),
    });
    update_rc_label(&app);

    // First launch from home: auto-enable right-click (flag set by setup_home).
    let rc_flag = config::data_dir().join("enable-rc");
    if rc_flag.exists() {
        let _ = std::fs::remove_file(&rc_flag);
        if !is_rightclick_enabled() {
            toggle_rightclick(true);
            update_rc_label(&app);
        }
    }

    // --- Browse ---
    // Runs on background thread to avoid rfd dialogs appearing behind Slint window
    {
        let w = app.as_weak();
        app.on_browse_clicked(move || {
            let a = match w.upgrade() { Some(a) => a, None => return };
            let current = a.get_source_text().to_string();
            let mut dialog = rfd::FileDialog::new();
            if !current.is_empty() && std::path::Path::new(&current).exists() {
                dialog = dialog.set_directory(&current);
            }
            // Folder picker must run on UI thread (Win32 requirement)
            if let Some(folder) = dialog.pick_folder() {
                let p = folder.to_string_lossy().to_string();
                let w = w.clone();
                // MessageDialog + config update on background thread (appears on top)
                std::thread::spawn(move || {
                    let mut cfg = config::load_config();
                    // L2: normalized dedup — config::same_path handles contracted/expanded
                    if !cfg.junctions.iter().any(|j| config::same_path(&j.source_path, &p)) {
                        let leaf = std::path::Path::new(&p).file_name()
                            .map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        let ar = rfd::MessageDialog::new()
                            .set_title("Add Folder")
                            .set_description(&format!("Add '{}' to sync list?\n\nEnable auto-restore?", leaf))
                            .set_buttons(rfd::MessageButtons::YesNo)
                            .show() == rfd::MessageDialogResult::Yes;
                        cfg.junctions.push(config::Junction {
                            source_path: p.clone(), auto_restore: ar, created: synclog::timestamp(), is_game: false,
                        });
                        if !config::save_config(&cfg) {
                            rfd::MessageDialog::new()
                                .set_title("Error")
                                .set_description("Could not save the config (disk full or OneDrive lock?) — the folder was NOT added.")
                                .set_buttons(rfd::MessageButtons::Ok)
                                .show();
                            return;
                        }

                        // Immediately backup the newly added folder — don't wait for interval
                        crate::synclog::write_progress(&format!("Compressing {}...", leaf));
                        let (ok, msg) = crate::sync::sync_pair_to_cloud(&p, &cfg.excluded_names, cfg.max_versions, true);
                        crate::synclog::write_progress("");
                        if ok { crate::health::write_status(1, 0, 0, &[]); }
                        w.upgrade_in_event_loop(move |a| {
                            a.set_source_text(p.into());
                            a.set_status_text(msg.into());
                            refresh_folders(&a);
                        }).ok();
                    } else {
                        // Already in config — just update UI
                        w.upgrade_in_event_loop(move |a| {
                            a.set_source_text(p.into());
                            refresh_folders(&a);
                        }).ok();
                    }
                });
            }
        });
    }

    // --- Backup Folder ---
    {
        let w = app.as_weak();
        app.on_link_clicked(move || {
            let a = match w.upgrade() { Some(a) => a, None => return };
                let path = a.get_source_text().to_string();
                if path.is_empty() || !std::path::Path::new(&path).exists() {
                    a.set_status_text("Invalid source folder.".into());
                    return;
                }
                let cfg = config::load_config();
                // L2: normalized dedup
                if cfg.junctions.iter().any(|j| config::same_path(&j.source_path, &path)) {
                    let leaf_name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    a.set_status_text(format!("Compressing {}...", leaf_name).into());
                    a.set_operation_running(true);
                    let w2 = w.clone();
                    std::thread::spawn(move || {
                        crate::synclog::write_progress(&format!("Compressing {}...", leaf_name));
                        let (ok, msg) = sync::sync_pair_to_cloud(&path, &cfg.excluded_names, cfg.max_versions, true);
                        crate::synclog::write_progress("");
                        if ok { health::write_status(1, 0, 0, &[]); }
                        w2.upgrade_in_event_loop(move |a| {
                            a.set_operation_running(false); a.set_status_text(msg.into());
                        }).ok();
                    });
                    return;
                }
                let leaf = std::path::Path::new(&path)
                    .file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let ar = rfd::MessageDialog::new()
                    .set_title("Auto-Restore")
                    .set_description(&format!("Enable auto-restore for '{}'?", leaf))
                    .set_buttons(rfd::MessageButtons::YesNo)
                    .show() == rfd::MessageDialogResult::Yes;
                let mut c2 = config::load_config();
                c2.junctions.retain(|j| j.source_path != path);
                c2.junctions.push(config::Junction {
                    source_path: path.clone(), auto_restore: ar, created: synclog::timestamp(), is_game: false,
                });
                if !config::save_config(&c2) {
                    rfd::MessageDialog::new()
                        .set_title("Error")
                        .set_description("Could not save the config (disk full or OneDrive lock?) — the folder was NOT added.")
                        .set_buttons(rfd::MessageButtons::Ok)
                        .show();
                    return;
                }
                a.set_operation_running(true);
                a.set_status_text(format!("Compressing {}...", leaf).into());
                let w3 = w.clone();
                std::thread::spawn(move || {
                    crate::synclog::write_progress(&format!("Compressing {}...", leaf));
                    let (ok, msg) = sync::sync_pair_to_cloud(&path, &c2.excluded_names, c2.max_versions, true);
                    crate::synclog::write_progress("");
                    if ok { health::write_status(1, 0, 0, &[]); }
                    w3.upgrade_in_event_loop(move |a| {
                        refresh_folders(&a);
                        a.set_operation_running(false);
                        a.set_status_text(msg.into());
                    }).ok();
                });
        });
    }

    // --- Restore Saved ---
    {
        let w = app.as_weak();
        app.on_restore_clicked(move || {
            let count_cfg = config::load_config();
            if count_cfg.junctions.is_empty() { return; }

            let a = match w.upgrade() { Some(a) => a, None => return };

            // If a folder is selected, restore ONLY that one. Otherwise restore all.
            let selected = a.get_selected_index();
            let to_restore: Vec<String> = if selected >= 0 && (selected as usize) < count_cfg.junctions.len() {
                vec![count_cfg.junctions[selected as usize].source_path.clone()]
            } else {
                // No folder selected — confirm before restoring ALL
                let count = count_cfg.junctions.len();
                let confirm = rfd::MessageDialog::new()
                    .set_title("Restore All")
                    .set_description(&format!("No folder selected.\n\nRestore ALL {} folders?", count))
                    .set_buttons(rfd::MessageButtons::YesNo)
                    .show() == rfd::MessageDialogResult::Yes;
                if !confirm { return; }
                count_cfg.junctions.iter().map(|j| j.source_path.clone()).collect()
            };

            // PRE-CHECK: skip folders that fail (permissions, corrupt archive).
            // Restore the rest. Each folder is still atomic (never half-written).
            let failures = crate::sync::pre_check_restore(&to_restore);
            let mut skipped_msg = String::new();
            let to_restore: Vec<String> = if failures.is_empty() {
                to_restore
            } else if failures.len() >= to_restore.len() {
                // ALL failed — nothing to restore
                let msg = format!("RESTORE ABORTED — all {} folder(s) failed pre-check:

{}",
                    failures.len(),
                    failures.iter().map(|(name, reason)| format!("  {}: {}", name, reason)).collect::<Vec<_>>().join("
"));
                a.set_restore_msg(msg.into());
                a.set_restore_failed(true);
                a.set_restore_done(true);
                a.set_restore_visible(true);
                return;
            } else {
                // Some failed — filter them out, save reasons for final message
                let skipped: Vec<String> = failures.iter().map(|(n, r)| format!("{}: {}", n, r)).collect();
                skipped_msg = skipped.join("; ");
                to_restore.into_iter().filter(|path| {
                    let leaf = std::path::Path::new(path).file_name()
                        .map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    !failures.iter().any(|(n, _)| n == &leaf)
                }).collect()
            };
            let junctions = to_restore;
            a.set_restore_msg("Preparing...".into());
            a.set_restore_visible(true);
            a.set_restore_done(false);
            let total = junctions.len();
            let w2 = w.clone();
            std::thread::spawn(move || {
                let mut count = 0;
                let mut failures: Vec<String> = vec![];
                for (i, source) in junctions.iter().enumerate() {
                    let leaf = std::path::Path::new(source).file_name()
                        .map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    let prog = format!("Decompressing {} ({} of {})...", leaf, i + 1, total);
                    w2.upgrade_in_event_loop(move |a| { a.set_restore_msg(prog.into()); }).ok();
                    let stop_hb = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let stop_hb2 = stop_hb.clone();
                    let w3 = w2.clone();
                    std::thread::spawn(move || {
                        while !stop_hb2.load(std::sync::atomic::Ordering::Relaxed) {
                            let p = crate::synclog::read_progress();
                            if !p.is_empty() {
                                let p2 = p.clone();
                                w3.upgrade_in_event_loop(move |a| { a.set_restore_msg(p2.into()); }).ok();
                            }
                            std::thread::sleep(std::time::Duration::from_millis(500));
                        }
                    });
                    let (ok, reason) = sync::restore_pair_from_cloud(source);
                    stop_hb.store(true, std::sync::atomic::Ordering::Relaxed);
                    crate::synclog::write(&format!("  [RESTORE] {} — {} — {}", if ok { "OK" } else { "FAIL" }, leaf, reason));
                    if ok { count += 1; }
                    else { failures.push(format!("{}: {}", leaf, reason)); }
                }
                // Any successful restore should set the migration marker —
                // the health timer (every 30s) will check it and run migration
                // automatically when the game creates a new ID folder.
                if count > 0 {
                    crate::sync::set_migration_pending();
                }
                let (msg, failed) = if count == total && failures.is_empty() && skipped_msg.is_empty() {
                    (format!("Restored {} folder(s).", count), false)
                } else {
                    let mut m = format!("Restored {} of {}.", count, total);
                    if !skipped_msg.is_empty() {
                        m.push_str(&format!("

Skipped:
{}", skipped_msg));
                    }
                    if !failures.is_empty() {
                        m.push_str(&format!("

Failed: {}", failures.join(", ")));
                    }
                    (m, failures.len() > 0)
                };
                w2.upgrade_in_event_loop(move |a| {
                    a.set_restore_msg(msg.into());
                    a.set_restore_failed(failed);
                    a.set_restore_done(true);
                }).ok();
            });
        });
    }

    // --- Toggle Auto-Restore ---
    {
        let w = app.as_weak();
        app.on_toggle_clicked(move || {
            let a = match w.upgrade() { Some(a) => a, None => return };
                let idx = a.get_selected_index();
                if idx < 0 {
                    rfd::MessageDialog::new()
                        .set_title("Toggle")
                        .set_description("Select a folder from the list first.")
                        .set_buttons(rfd::MessageButtons::Ok)
                        .show();
                    return;
                }
                let mut cfg = config::load_config();
                let i = idx as usize;
                if i >= cfg.junctions.len() { return; }
                cfg.junctions[i].auto_restore = !cfg.junctions[i].auto_restore;
                let ar = cfg.junctions[i].auto_restore;
                config::save_config(&cfg);
                refresh_folders(&a);
                a.set_status_text(format!("Auto-restore {}.", if ar { "ON" } else { "OFF" }).into());
                a.set_selected_index(idx);
        });
    }

    // --- Remove (button + Tools menu) ---
    {
        let w = app.as_weak();
        app.on_remove_clicked(move || {
            let a = match w.upgrade() { Some(a) => a, None => return };
            let idx = a.get_selected_index();
            if idx < 0 {
                rfd::MessageDialog::new()
                    .set_title("Remove")
                    .set_description("Select a folder from the list first.")
                    .set_buttons(rfd::MessageButtons::Ok)
                    .show();
                return;
            }
            let mut cfg = config::load_config();
            let i = idx as usize;
            if i >= cfg.junctions.len() { return; }
            let source_path = cfg.junctions[i].source_path.clone();
            let name = std::path::Path::new(&source_path)
                .file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let confirm = rfd::MessageDialog::new()
                .set_title("Confirm Remove")
                .set_description(&format!("Remove '{}' from sync list?", name))
                .set_buttons(rfd::MessageButtons::YesNo)
                .show() == rfd::MessageDialogResult::Yes;
            if !confirm { return; }
            // L-9: re-resolve by PATH before mutating — the index may point at a
            // different junction if the config changed since selection (removes
            // the wrong pair's entry).
            let mut cfg = config::load_config(); // fresh load: dialogs took time
            match cfg.junctions.iter().position(|j| config::same_path(&j.source_path, &source_path)) {
                Some(pos) => {
                    cfg.junctions.remove(pos);
                    if !config::save_config(&cfg) {
                        rfd::MessageDialog::new()
                            .set_title("Error")
                            .set_description("Could not save the config — the folder may still be in the list.")
                            .set_buttons(rfd::MessageButtons::Ok)
                            .show();
                        return;
                    }
                }
                None => return, // already gone — nothing to do
            }
            a.set_selected_index(-1);
            refresh_folders(&a);

            // Ask if user wants to also delete the backup files (C2: keyed dirs)
            let backup_dir = config::backup_dir_for(&source_path);
            let versions_dir = config::trash_path_for(&source_path);
            let has_backup = !name.is_empty() && (backup_dir.exists() || versions_dir.exists());
            if has_backup {
                let delete_backup = rfd::MessageDialog::new()
                    .set_title("Delete Backup?")
                    .set_description(&format!("'{}' removed from sync list.\n\nAlso delete the backup files?", name))
                    .set_buttons(rfd::MessageButtons::YesNo)
                    .show() == rfd::MessageDialogResult::Yes;
                if delete_backup {
                    // M-3: deletion under the global lock — a scheduled sync could
                    // otherwise race a snapshot hardlink into a dir being deleted.
                    let mut deleted = false;
                    match sync::acquire_pair_lock(10_000) {
                        Some(_lock) => {
                            let _ = std::fs::remove_dir_all(&backup_dir);
                            let _ = std::fs::remove_dir_all(&versions_dir);
                            deleted = true;
                        }
                        None => {
                            rfd::MessageDialog::new()
                                .set_title("Busy")
                                .set_description("A sync is running — the backup files could not be deleted now. Remove it again when the sync finishes.")
                                .set_buttons(rfd::MessageButtons::Ok)
                                .show();
                        }
                    }
                    if deleted {
                        rfd::MessageDialog::new()
                            .set_title("Done")
                            .set_description(&format!("'{}' and its backup fully removed.", name))
                            .set_buttons(rfd::MessageButtons::Ok)
                            .show();
                    }
                } else {
                    rfd::MessageDialog::new()
                        .set_title("Removed")
                        .set_description(&format!("'{}' removed from sync list.\nBackup files kept.", name))
                        .set_buttons(rfd::MessageButtons::Ok)
                        .show();
                }
            } else {
                rfd::MessageDialog::new()
                    .set_title("Removed")
                    .set_description(&format!("'{}' removed from sync list.", name))
                    .set_buttons(rfd::MessageButtons::Ok)
                    .show();
            }
        });
    }

    // --- View Sync Log ---
    app.on_log_clicked(|| {
        let log = synclog::read_tail(50);
        let display = if log.is_empty() { "No log entries yet.".to_string() } else { log };
        rfd::MessageDialog::new()
            .set_title("Sync Log (last 50 lines)")
            .set_description(&display)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
    });

    // --- Right-Click Sync Toggle ---
    {
        let w = app.as_weak();
        app.on_rightclick_clicked(move || {
            let enabled = is_rightclick_enabled();
            let new_state = !enabled;
            toggle_rightclick(new_state);
            let actually_enabled = is_rightclick_enabled();
            rfd::MessageDialog::new()
                .set_title("Right-Click Sync")
                .set_description(if actually_enabled {
                    "Right-click sync ENABLED. Right-click any folder to sync it."
                } else if new_state {
                    "FAILED to enable. Try running as Administrator."
                } else {
                    "Right-click sync removed."
                })
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
            w.upgrade_in_event_loop(|a| { update_rc_label(&a); }).ok();
        });
    }

    // --- Junction Health Check ---
    app.on_health_clicked(|| {
        let cfg = config::load_config();
        let total = cfg.junctions.len();
        if total == 0 {
            rfd::MessageDialog::new()
                .set_title("Backup Health Check")
                .set_description("No folders configured yet.")
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
            return;
        }
        let mut details = String::new();
        let mut ok_count = 0;
        // STALE check: based on when the SYNC LAST RAN (sync-status.json mtime),
        // not individual backup file age. If the sync ran within the interval,
        // all folders were checked — unchanged folders are NOT stale just because
        // their backup file is old (change detection correctly skipped them).
        let sync_age_hours: i32 = std::fs::metadata(health::status_path())
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .map(|e| (e.as_secs() / 3600) as i32)
            .unwrap_or(999);
        let sync_stale = sync_age_hours > (cfg.sync_interval_minutes / 60) * 2;
        for j in &cfg.junctions {
            let leaf = std::path::Path::new(&j.source_path)
                .file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let backup = config::backup_file_for(&j.source_path);
            let source_exists = std::path::Path::new(&j.source_path).exists();
            if backup.exists() {
                ok_count += 1;
                let size_mb = std::fs::metadata(&backup).map(|m| m.len() as f64 / 1_048_576.0).unwrap_or(0.0);
                let age_hours = std::fs::metadata(&backup).ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.elapsed().ok())
                    .map(|e| e.as_secs() / 3600)
                    .unwrap_or(999);
                let age_str = if age_hours < 24 {
                    format!("{}h ago", age_hours)
                } else {
                    format!("{}d ago", age_hours / 24)
                };
                let src_status = if source_exists { "source OK" } else { "source MISSING" };
                let stale = if sync_stale { " \u{26a0} STALE" } else { "" };
                details.push_str(&format!("\n  {} \u{2014} {:.1} MB, {}, {}{}", leaf, size_mb, age_str, src_status, stale));
            } else {
                details.push_str(&format!("\n  {} \u{2014} NO BACKUP", leaf));
            }
        }
        rfd::MessageDialog::new()
            .set_title("Backup Health Check")
            .set_description(&format!("{} of {} folders backed up:{}", ok_count, total, details))
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
    });

    // --- Export Configuration ---
    app.on_export_clicked(|| {
        let mut cfg = config::load_config();
        for j in &mut cfg.junctions {
            j.source_path = crate::pathutil::contract(&j.source_path);
        }
        if let Some(path) = rfd::FileDialog::new()
            .set_file_name("lrgex-restore-config.json")
            .add_filter("JSON", &["json"])
            .save_file()
        {
            if let Ok(data) = serde_json::to_string_pretty(&cfg) {
                let _ = std::fs::write(&path, data);
                rfd::MessageDialog::new()
                    .set_title("Export Complete")
                    .set_description("Configuration exported successfully.")
                    .set_buttons(rfd::MessageButtons::Ok)
                    .show();
            }
        }
    });

    // --- Import Configuration ---
    {
        let w = app.as_weak();
        app.on_import_clicked(move || {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("JSON", &["json"])
                .pick_file()
            {
                if let Ok(raw) = std::fs::read_to_string(&path) {
                    let raw = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
                    match serde_json::from_str::<config::Config>(raw) {
                        Ok(cfg) => {
                            let imported = config::save_config(&cfg);
                            w.upgrade_in_event_loop(|a| { refresh_folders(&a); }).ok();
                            rfd::MessageDialog::new()
                                .set_title(if imported { "Import Complete" } else { "Import Failed" })
                                .set_description(if imported {
                                    "Configuration imported successfully."
                                } else {
                                    "Import parsed but could not be saved (disk full or OneDrive lock?) — NOTHING changed."
                                })
                                .set_buttons(rfd::MessageButtons::Ok)
                                .show();
                        }
                        Err(_) => {
                            rfd::MessageDialog::new()
                                .set_title("Import Failed")
                                .set_description("Could not read configuration file. Invalid format.")
                                .set_buttons(rfd::MessageButtons::Ok)
                                .show();
                        }
                    }
                }
            }
        });
    }

    // --- Set Sync Interval ---
    {
        let w = app.as_weak();
        app.on_interval_clicked(move || {
            if let Some(a) = w.upgrade() {
                let cfg = config::load_config();
                a.set_input_title("Sync Interval".into());
                a.set_input_mode(0);
                a.set_input_prompt(format!("Current: {} min. Enter new interval (1+):", cfg.sync_interval_minutes).into());
                a.set_input_value(cfg.sync_interval_minutes.to_string().into());
                a.set_input_visible(true);
            }
        });
    }

    // --- Set Max Versions ---
    {
        let w = app.as_weak();
        app.on_maxversions_clicked(move || {
            if let Some(a) = w.upgrade() {
                let cfg = config::load_config();
                a.set_input_title("Max Versions".into());
                a.set_input_mode(1);
                a.set_input_prompt(format!("Current: {}. Enter new limit (1+):", cfg.max_versions).into());
                a.set_input_value(cfg.max_versions.to_string().into());
                a.set_input_visible(true);
            }
        });
    }

    // --- Manage Exclusions ---
    {
        let w = app.as_weak();
        app.on_exclusions_clicked(move || {
            if let Some(a) = w.upgrade() {
                let cfg = config::load_config();
                let current = cfg.excluded_names.join(", ");
                a.set_input_title("Manage Exclusions".into());
                a.set_input_mode(2);
                a.set_input_prompt("Enter names (comma-separated), or clear:".into());
                a.set_input_value(current.into());
                a.set_input_visible(true);
            }
        });
    }

    // --- Versions (per folder) ---
    // Shared state for version restore (paths + source)
    let version_state: std::rc::Rc<std::cell::RefCell<(Vec<std::path::PathBuf>, String)>> =
        std::rc::Rc::new(std::cell::RefCell::new((vec![], String::new())));

    // --- Versions button: populate list + show panel ---
    {
        let w = app.as_weak();
        let vs = version_state.clone();
        app.on_versions_clicked(move |idx| {
            let a = match w.upgrade() { Some(a) => a, None => return };
            let cfg = config::load_config();
            let i = idx as usize;
            if i >= cfg.junctions.len() { return; }
            let source = cfg.junctions[i].source_path.clone();
            let leaf = std::path::Path::new(&source)
                .file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let versions_folder = config::trash_path_for(&source);

            let mut snapshots: Vec<(String, std::path::PathBuf)> = vec![];
            if let Ok(entries) = std::fs::read_dir(&versions_folder) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    // L-2: validate AS BYTES before slicing — a multibyte-UTF-8
                    // 15-char name would panic the GUI event loop mid-slice.
                    let b = name.as_bytes();
                    if b.len() == 15 && entry.path().is_dir()
                        && b[..8].iter().all(|c| c.is_ascii_digit())
                        && b[8] == b'_'
                        && b[9..].iter().all(|c| c.is_ascii_digit()) {
                        let formatted = format!(
                            "{}-{}-{} {}:{}:{}",
                            &name[0..4], &name[4..6], &name[6..8],
                            &name[9..11], &name[11..13], &name[13..15]
                        );
                        snapshots.push((formatted, entry.path()));
                    }
                }
            }
            snapshots.sort_by(|a, b| b.0.cmp(&a.0)); // newest first

            if snapshots.is_empty() {
                rfd::MessageDialog::new()
                    .set_title("Versions")
                    .set_description("No versions saved for this folder yet.")
                    .set_buttons(rfd::MessageButtons::Ok)
                    .show();
                return;
            }

            // Store paths + source in shared state
            let paths: Vec<std::path::PathBuf> = snapshots.iter().map(|(_, p)| p.clone()).collect();
            *vs.borrow_mut() = (paths, source);

            // Populate Slint model
            let model = slint::VecModel::from_iter(
                snapshots.iter().map(|(display, _)| VersionEntry { display: display.clone().into() })
            );
            a.set_versions_title(format!("Versions ({})", leaf).into());
            a.set_versions_list(slint::ModelRc::new(model));
            a.set_selected_version(-1);
            a.set_versions_visible(true);
        });
    }

    // --- Restore selected version ---
    {
        let w = app.as_weak();
        let vs = version_state.clone();
        app.on_restore_version(move || {
            let a = match w.upgrade() { Some(a) => a, None => return };
            let idx = a.get_selected_version();
            if idx < 0 {
                rfd::MessageDialog::new()
                    .set_title("Restore")
                    .set_description("Select a version first.")
                    .set_buttons(rfd::MessageButtons::Ok)
                    .show();
                return;
            }
            let (paths, source) = vs.borrow().clone();
            let i = idx as usize;
            if i >= paths.len() { return; }

            let confirm = rfd::MessageDialog::new()
                .set_title("Confirm Restore")
                .set_description("Restore entire folder from this version?\nThis will overwrite current files.")
                .set_buttons(rfd::MessageButtons::YesNo)
                .show() == rfd::MessageDialogResult::Yes;
            if !confirm { return; }

            let (ok, msg) = sync::restore_snapshot(&paths[i], &source);
            a.set_versions_visible(false);
            rfd::MessageDialog::new()
                .set_title(if ok { "Restored" } else { "Restore Failed" })
                .set_description(if ok { "Folder restored successfully.".into() } else { format!("Error: {}", msg) })
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
        });
    }

    // --- Preview version (extract to temp + open Explorer) ---
    {
        let w = app.as_weak();
        let vs = version_state.clone();
        app.on_preview_version(move || {
            let a = match w.upgrade() { Some(a) => a, None => return };
            let idx = a.get_selected_version();
            if idx < 0 {
                rfd::MessageDialog::new()
                    .set_title("Preview")
                    .set_description("Select a version first.")
                    .set_buttons(rfd::MessageButtons::Ok)
                    .show();
                return;
            }
            let (paths, _) = vs.borrow().clone();
            let i = idx as usize;
            if i >= paths.len() { return; }

            // Find .tar.zst inside the snapshot directory
            let mut archive_path = None;
            if let Ok(entries) = std::fs::read_dir(&paths[i]) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().map(|e| e == "zst").unwrap_or(false) {
                        archive_path = Some(p);
                        break;
                    }
                }
            }

            match archive_path {
                Some(path) => {
                    a.set_status_text("Opening preview...".into());
                    // Check for WinRAR or 7-Zip to browse in-place
                    let archiver = [
                        r"C:\Program Files\WinRAR\WinRAR.exe",
                        r"C:\Program Files (x86)\WinRAR\WinRAR.exe",
                        r"C:\Program Files\7-Zip\7zFM.exe",
                        r"C:\Program Files (x86)\7-Zip\7zFM.exe",
                    ].iter().map(std::path::PathBuf::from).find(|p| p.exists());
                    if let Some(app) = &archiver {
                        let _ = std::process::Command::new(app)
                            .arg(&path)
                            .spawn();
                        a.set_status_text("Opened in archiver.".into());
                    } else {
                        // Fallback: extract to temp + open Explorer
                        a.set_status_text("Extracting preview...".into());
                        let w2 = w.clone();
                        std::thread::spawn(move || {
                            let preview_dir = std::env::temp_dir().join("lrgex-preview");
                            let _ = std::fs::remove_dir_all(&preview_dir);
                            let _ = std::fs::create_dir_all(&preview_dir);
                            if sync::decompress_archive(&path, &preview_dir).0 {
                                let _ = std::process::Command::new("explorer.exe")
                                    .arg(&preview_dir)
                                    .spawn();
                                w2.upgrade_in_event_loop(|a| {
                                    a.set_status_text("Preview opened in Explorer.".into());
                                }).ok();
                            } else {
                                w2.upgrade_in_event_loop(|a| {
                                    a.set_status_text("Preview failed.".into());
                                }).ok();
                            }
                        });
                    }
                }
                None => {
                    rfd::MessageDialog::new()
                        .set_title("Preview")
                        .set_description("No archive found in this snapshot.")
                        .set_buttons(rfd::MessageButtons::Ok)
                        .show();
                }
            }
        });
    }

    // --- Close versions panel ---
    {
        let w = app.as_weak();
        app.on_close_versions(move || {
            if let Some(a) = w.upgrade() {
                a.set_versions_visible(false);
            }
        });
    }

    // Shared cache for last health result (avoids schtasks on every 3s tick)
    let health_cache: std::sync::Arc<std::sync::Mutex<(slint::SharedString, slint::Color)>> =
        std::sync::Arc::new(std::sync::Mutex::new((" Checking...".into(), slint::Color::from_rgb_u8(76, 175, 80))));

    // --- Input dialog OK handler (routes by input-mode) ---
    {
        let w = app.as_weak();
        app.on_input_ok(move || {
            let a = match w.upgrade() { Some(a) => a, None => return };
            let val = a.get_input_value().to_string();
            let mode = a.get_input_mode();
            a.set_input_visible(false);

            match mode {
                0 => { // Sync Interval
                    if let Ok(mins) = val.trim().parse::<i32>() {
                        if mins >= 1 {
                            let mut c2 = config::load_config();
                            c2.sync_interval_minutes = mins;
                            config::save_config(&c2);
                            // H-1: verify registration actually succeeded before
                            // claiming it — a failed schtasks silently kept the
                            // OLD interval while telling the user "Set to X".
                            let (tx, rx) = std::sync::mpsc::channel();
                            std::thread::spawn(move || { tx.send(sync::register_sync_task(mins)).ok(); });
                            let registered = rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap_or(false);
                            let saved = config::load_config().sync_interval_minutes == mins;
                            rfd::MessageDialog::new()
                                .set_title("Sync Interval")
                                .set_description(if registered && saved {
                                    format!("Set to {} minute(s).", mins)
                                } else if !saved {
                                    "Config could not be saved (disk full or OneDrive lock?) — interval NOT changed.".into()
                                } else {
                                    "Failed to register the scheduled task — interval NOT applied.".into()
                                })
                                .set_buttons(rfd::MessageButtons::Ok)
                                .show();
                        } else {
                            rfd::MessageDialog::new().set_title("Invalid").set_description("Enter 1 or more.").set_buttons(rfd::MessageButtons::Ok).show();
                        }
                    } else {
                        rfd::MessageDialog::new().set_title("Invalid").set_description("Enter a whole number.").set_buttons(rfd::MessageButtons::Ok).show();
                    }
                }
                1 => { // Max Versions
                    if let Ok(n) = val.trim().parse::<i32>() {
                        if n >= 1 {
                            let mut c2 = config::load_config();
                            c2.max_versions = n;
                            config::save_config(&c2);
                            rfd::MessageDialog::new()
                                .set_title("Max Versions")
                                .set_description(&format!("Will keep last {} version(s).", n))
                                .set_buttons(rfd::MessageButtons::Ok)
                                .show();
                        } else {
                            rfd::MessageDialog::new().set_title("Invalid").set_description("Enter 1 or more.").set_buttons(rfd::MessageButtons::Ok).show();
                        }
                    } else {
                        rfd::MessageDialog::new().set_title("Invalid").set_description("Enter a whole number.").set_buttons(rfd::MessageButtons::Ok).show();
                    }
                }
                2 => { // Exclusions
                    let trimmed = val.trim();
                    let mut c2 = config::load_config();
                    c2.excluded_names = if trimmed.is_empty() { vec![] } else {
                        trimmed.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
                    };
                    let result = if c2.excluded_names.is_empty() { "(none)".to_string() } else { c2.excluded_names.join(", ") };
                    config::save_config(&c2);
                    rfd::MessageDialog::new()
                        .set_title("Exclusions Updated")
                        .set_description(&format!("Exclusions: {}", result))
                        .set_buttons(rfd::MessageButtons::Ok)
                        .show();
                }
                3 => { // Unlink from Windows
                    if val.trim().eq_ignore_ascii_case("yes") {
                        let _ = std::fs::remove_file(config::data_dir().join("home"));
                        let _ = std::fs::remove_file(config::script_dir().join(".lrgex-home"));
                        config::clear_canonical_home();
                        let bat_path = std::env::temp_dir().join("lrgex-cleanup.bat");
                        let bat = "@echo off\r\nping 127.0.0.1 -n 3 > nul\r\nschtasks /Delete /TN \"LRGEX-Restore-Rust\" /F >nul 2>&1\r\nreg delete \"HKCU\\Software\\Classes\\Directory\\shell\\LRGEXRestore\" /f >nul 2>&1\r\ndel \"%~f0\"\r\n";
                        let _ = std::fs::write(&bat_path, bat);
                        rfd::MessageDialog::new()
                            .set_title("Unlinked")
                            .set_description("Cleanup will finish in a moment.\n\nYou can now safely delete this folder.")
                            .set_buttons(rfd::MessageButtons::Ok)
                            .show();
                        let _ = std::process::Command::new("cmd.exe")
                            .args(["/c", bat_path.to_str().unwrap_or("")])
                            .creation_flags(0x08000000u32)
                            .spawn();
                        std::process::exit(0);
                    } else {
                        rfd::MessageDialog::new().set_title("Unlink").set_description("Cancelled.").set_buttons(rfd::MessageButtons::Ok).show();
                    }
                }
                _ => {}
            }
        });
    }

    // --- Input dialog Cancel handler ---
    {
        let w = app.as_weak();
        app.on_input_cancel(move || {
            if let Some(a) = w.upgrade() {
                a.set_input_visible(false);
            }
        });
    }

    // Live progress: 500ms heartbeat with spinner + liveness + folder-list refresh.
    let progress_timer = slint::Timer::default();
    {
        let w = app.as_weak();
        let cache = health_cache.clone();
        let cfg_path = config::config_path();
        let mut last_folder_count: Option<usize> = None;
        let mut spin: usize = 0;
        progress_timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(500), move || {
            let Some(a) = w.upgrade() else { return };

            // Folder-list refresh: reload only if the junction COUNT changed (folder added/removed).
            // Watching mtime loops — load_config's portable-path migration re-saves the file
            // every call for paths it can't contract (e.g. E:\), which flips the mtime.
            let count = std::fs::read_to_string(&cfg_path).ok()
                .and_then(|r| serde_json::from_str::<serde_json::Value>(&r).ok())
                .and_then(|v| v.get("Junctions").and_then(|j| j.as_array()).map(|a| a.len()))
                .unwrap_or(0);
            if Some(count) != last_folder_count {
                last_folder_count = Some(count);
                refresh_folders(&a);
            }

            spin = (spin + 1) % 8;
            const FRAMES: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

            match synclog::read_status() {
                Some(s) if s.phase < 3 => {
                    // Liveness: heartbeat stale AND pid gone => the sync process died.
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                    if now.saturating_sub(s.heartbeat) > 15 && !synclog::is_pid_alive(s.pid) {
                        a.set_health_text(" Sync stopped unexpectedly ".into());
                        a.set_health_color(slint::Color::from_rgb_u8(200, 30, 30));
                        return;
                    }
                    let pct = if s.bytes_total > 0 { s.bytes_done * 100 / s.bytes_total } else { 0 };
                    let text = match s.phase {
                        0 => format!(" {} Scanning {} — {} files ", FRAMES[spin], s.label, s.files_done),
                        2 => format!(" {} Finalizing {} ", FRAMES[spin], s.label),
                        _ => format!(
                            " {} {} — {}%  ({}/{} MB, {:.0} MB/s, {}{}) ",
                            FRAMES[spin], s.label, pct,
                            s.bytes_done / 1_048_576, s.bytes_total / 1_048_576,
                            s.bytes_done as f64 / 1_048_576.0 / s.elapsed,
                            fmt_dur(s.elapsed),
                            if s.eta > 0 { format!(", ~{} left", fmt_dur(s.eta as f64)) } else { String::new() }
                        ),
                    };
                    a.set_health_text(text.into());
                    a.set_health_color(slint::Color::from_rgb_u8(200, 140, 0));
                }
                _ => {
                    let cached = cache.lock().unwrap();
                    a.set_health_text(cached.0.clone());
                    a.set_health_color(cached.1);
                }
            }
        });
    }

    // Periodic full health refresh every 30 seconds
    let health_timer = slint::Timer::default();
    {
        let w = app.as_weak();
        let cache = health_cache.clone();
        health_timer.start(slint::TimerMode::Repeated, std::time::Duration::from_secs(30), move || {
            // L-4: get_health() spawns powershell.exe (0.3-1s) — doing that on the
            // UI thread stalls the event loop every 30s. Do the work on a thread,
            // post the result back via the event loop.
            let w2 = w.clone();
            let cache_tick = cache.clone(); // FnMut timer: fresh Arc clone per tick
            std::thread::spawn(move || {
                let h = health::get_health();
                let text: slint::SharedString = format!(" {} - {} ", h.label, h.reason).into();
                let color = match h.status.as_str() {
                    "GREEN" => slint::Color::from_rgb_u8(76, 175, 80),
                    "AMBER" => slint::Color::from_rgb_u8(200, 140, 0),
                    _ => slint::Color::from_rgb_u8(200, 30, 30),
                };
                let syncing = synclog::read_status().map_or(false, |s| s.phase < 3);
                let _ = w2.upgrade_in_event_loop(move |a| {
                    // Cache write happens on the UI thread.
                    *cache_tick.lock().unwrap() = (text.clone(), color);
                    // Don't overwrite health bar with "OK" while a sync runs —
                    // the progress timer (3s) owns the display then.
                    if !syncing {
                        a.set_health_text(text);
                        a.set_health_color(color);
                    }
                });
            });

            // Migration check: only spawn thread if marker exists (cheap file check).
            // Zero threads in steady state.
            // run migration on a background thread. Mutex prevents races.
            // The user never needs to close/reopen — just restore + play.
            if crate::sync::migration_marker_exists() {
            let w_mig = w.clone();
            std::thread::spawn(move || {
                let migrations = crate::sync::run_migration_check();
                if migrations.is_empty() { return; }
                for m in &migrations {
                    crate::synclog::write(&format!("  [MIGRATE] {}", m));
                }
                if let Some(msg) = migrations.iter().find(|m| !m.starts_with("REFUSED")) {
                    let msg = msg.clone();
                    let _ = w_mig.upgrade_in_event_loop(move |a| {
                        a.set_health_text(msg.into());
                    });
                }
            });
            } // end migration_marker_exists gate
        });
    }

    // Check for updates 2 seconds after launch (one-shot)
    let update_timer = slint::Timer::default();
    update_timer.start(slint::TimerMode::SingleShot, std::time::Duration::from_secs(2), || {
        crate::update::check_for_updates();
    });

    // --- Unlink from Windows ---
    {
        let w = app.as_weak();
        app.on_uninstall_clicked(move || {
            if let Some(a) = w.upgrade() {
                a.set_input_title("Unlink from Windows".into());
                a.set_input_mode(3);
                a.set_input_prompt("Type 'yes' to confirm. Removes task + right-click + marker.\nBackups will NOT be deleted.".into());
                a.set_input_value("".into());
                a.set_input_visible(true);
            }
        });
    }

    // --- Restore OK (dismiss overlay) ---
    {
        let w = app.as_weak();
        app.on_restore_ok(move || {
            if let Some(a) = w.upgrade() {
                a.set_restore_visible(false);
                a.set_restore_done(false);
            }
        });
    }

    // Run all post-launch checks (migration, future checks, etc.)
    run_post_launch_checks(&app.as_weak());

    // Close guard: warn user if compression or restore is running
    let w_close = app.as_weak();
    app.window().on_close_requested(move || {
        let running = w_close.upgrade()
            .map(|a| a.get_operation_running() || a.get_restore_visible())
            .unwrap_or(false);
        if running {
            let what = if w_close.upgrade().map(|a| a.get_restore_visible()).unwrap_or(false) { "A restore" } else { "A backup" };
            let confirm = rfd::MessageDialog::new()
                .set_title("Operation in Progress")
                .set_description(&format!("{} is still running.\n\nClosing now will ABORT it.\n\nClose anyway?", what))
                .set_buttons(rfd::MessageButtons::YesNo)
                .show() == rfd::MessageDialogResult::Yes;
            if !confirm {
                return slint::CloseRequestResponse::KeepWindowShown;
            }
        }
        slint::CloseRequestResponse::HideWindow
    });

    // Disable maximize/resize — app looks bad maximized

    app.run().unwrap();
}

// ==================== POST-LAUNCH CHECKS ====================
// Orchestrator: runs all checks 3s after GUI launch on a background thread.
//
// HOW TO ADD A NEW CHECK:
//   1. Write a function: fn run_my_check(w: &slint::Weak<App>) { ... }
//   2. Add it to the list in run_post_launch_checks() below.
//   3. Each check is panic-isolated — one failing check won't kill the others.
//   4. If a check is slow (network, heavy I/O), spawn its OWN thread inside it
//      so it doesn't block checks after it.
//
fn run_post_launch_checks(weak: &slint::Weak<App>) {
    let w = weak.clone();
    std::thread::spawn(move || {
        // Let the UI settle before doing any work
        std::thread::sleep(std::time::Duration::from_secs(3));

        // Each check is wrapped in catch_unwind so a panic in one
        // doesn't silently disable the rest.
        use std::panic::{self, AssertUnwindSafe};

        // --- Current checks ---
        let _ = panic::catch_unwind(AssertUnwindSafe(|| { run_save_migration(&w); }));

        // --- Future checks go here ---
        // let _ = panic::catch_unwind(AssertUnwindSafe(|| { run_update_check(&w); }));
        // let _ = panic::catch_unwind(AssertUnwindSafe(|| { run_cleanup_check(&w); }));
    });
}

/// Check 1: Game save-ID migration.
/// Only fires if restore happened recently (marker file exists).
/// Migrates saves from old numeric ID folders to new empty ones.
/// Non-destructive: never overwrites, never deletes.
fn run_save_migration(w: &slint::Weak<App>) {
    let migrations = crate::sync::run_migration_check();
    if migrations.is_empty() { return; }

    // Log everything (including REFUSED) for debugging
    for m in &migrations {
        crate::synclog::write(&format!("  [MIGRATE] {}", m));
    }

    // Only show real migrations in health bar (not REFUSED — those are
    // non-game numeric dirs like node_modules/es-abstract/2015, normal noise)
    if let Some(msg) = migrations.iter().find(|m| !m.starts_with("REFUSED")) {
        let msg = msg.clone();
        let _ = w.upgrade_in_event_loop(move |a| {
            a.set_health_text(msg.into());
        });
    }
}

fn setup_home() {
    rfd::MessageDialog::new()
        .set_title("First Run Setup")
        .set_description("Pick the folder where LRGEX Restore will live.

RECOMMENDED: a folder inside a cloud service (OneDrive, Google Drive, etc.) so your backups survive a PC format.")
        .set_buttons(rfd::MessageButtons::Ok)
        .show();

    let home = rfd::FileDialog::new()
        .set_title("Select LRGEX Restore folder")
        .pick_folder();

    if let Some(home) = home {
        if let Ok(exe) = std::env::current_exe() {
            let exe_name = exe.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "LRGEXRestore.exe".into());
            let dest = home.join(&exe_name);
            let _ = std::fs::copy(&exe, &dest);
            let _ = std::fs::create_dir_all(home.join(".lrgex"));
            let _ = std::fs::write(home.join(".lrgex").join("home"), "LRGEX Restore Home");
            // Set canonical home in registry — ONE source of truth
            config::set_canonical_home(&home);

            let lrgex_dir = home.join(".lrgex");
            let _ = std::fs::create_dir_all(&lrgex_dir);
            let cfg_path = lrgex_dir.join("junction-config.json");
            if !cfg_path.exists() {
                if let Ok(data) = serde_json::to_string_pretty(&config::Config::default()) {
                    let _ = std::fs::write(&cfg_path, data);
                }
            }

            // Clean up any stray .lrgex created in the ORIGINAL location (Desktop/Downloads).
            let orig_lrgex = config::script_dir().join(".lrgex");
            if orig_lrgex != home.join(".lrgex") {
                let _ = std::fs::remove_dir_all(&orig_lrgex);
            }

            // Flag: tell the new exe (launching from home) to enable right-click by default.
            let _ = std::fs::write(home.join(".lrgex").join("enable-rc"), "");

            // Tell the user where to find the app.
            rfd::MessageDialog::new()
                .set_title("Setup Complete")
                .set_description(&format!(
                    "LRGEX Restore is now installed at:\n{}\n\nOpen it from there.\n\nYou can delete this copy ({}).",
                    home.display(),
                    exe.display()
                ))
                .set_buttons(rfd::MessageButtons::Ok)
                .show();

            let _ = std::process::Command::new(&dest).spawn();
        }
    }
}

fn refresh_folders(app: &App) {
    let mut cfg = config::load_config();
    let mut cfg_changed = false;

    let model = VecModel::from_iter(cfg.junctions.iter_mut().map(|j| {
        // Cached TRUE → skip scan entirely (confirmed game folder)
        // Uncached/FALSE → scan (bounded 300-dir cap), cache if game found
        let is_game = if j.is_game {
            true
        } else {
            let detected = crate::sync::is_game_folder(&j.source_path);
            if detected {
                j.is_game = true; // Cache: persist so future launches skip scan
                cfg_changed = true;
            }
            detected
        };
        let leaf = std::path::Path::new(&j.source_path)
            .file_name().map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| j.source_path.clone());
        FolderEntry {
            path: j.source_path.clone().into(),
            name: leaf.into(),
            auto_restore: if j.auto_restore { "ON" } else { "OFF" }.into(),
            is_game,
        }
    }));
    app.set_folders(slint::ModelRc::new(model));

    // Persist newly-detected game flags (TRUE only — never cache FALSE)
    if cfg_changed {
        let _ = config::save_config(&cfg);
    }
}

fn update_rc_label(app: &App) {
    let enabled = is_rightclick_enabled();
    app.set_rc_label(
        if enabled { "Right-Click Sync: ON   (click to disable)" }
        else { "Right-Click Sync: OFF   (click to enable)" }
        .into()
    );
}

fn is_rightclick_enabled() -> bool {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Classes\Directory\shell\LRGEXRestore").is_ok()
}

fn toggle_rightclick(enable: bool) {
    let sep = std::path::MAIN_SEPARATOR;
    let shell_path = format!("Software{}Classes{}Directory{}shell", sep, sep, sep);
    let shell = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .create_subkey(&shell_path)
        .map(|(k,_)| k);
    if enable {
        if let Ok(shell) = shell {
            // Create LRGEXRestore key with display name
            if let Ok((key,_)) = shell.create_subkey("LRGEXRestore") {
                let _ = key.set_value("", &"Add to LRGEX Restore");
                // Icon = the exe itself (icon embedded via winres)
                let exe = std::env::current_exe()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                if !exe.is_empty() {
                    let _ = key.set_value("Icon", &exe);
                }
                // Command subkey: exe -link "%V"
                if let Ok((cmd,_)) = key.create_subkey("command") {
                    let cmd_str = format!("\"{}\" -link \"%V\"", exe);
                    let _ = cmd.set_value("", &cmd_str);
                }
            }
        }
    } else {
        if let Ok(shell) = shell {
            let _ = shell.delete_subkey_all("LRGEXRestore");
        }
    }


}
