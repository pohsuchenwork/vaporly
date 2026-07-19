//! Tray menu strings (v2: English only).
//!
//! v1 generated this file at build time from every frontend locale; v2 ships
//! one language, so the strings are plain consts. If a menu item is added,
//! add a field here and use it in tray.rs.

pub struct TrayStrings {
    pub settings: &'static str,
    pub check_updates: &'static str,
    pub copy_last_transcript: &'static str,
    pub quit: &'static str,
    pub cancel: &'static str,
}

pub const TRAY_STRINGS: TrayStrings = TrayStrings {
    settings: "Settings...",
    check_updates: "Check for Updates...",
    copy_last_transcript: "Copy Last Transcript",
    quit: "Quit",
    cancel: "Cancel",
};
