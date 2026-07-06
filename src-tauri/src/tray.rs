//! System tray + recording indicator (audio-graph-a156, epic 5c24).
//!
//! Builds a single [`tauri::tray::TrayIcon`] during setup with:
//!   - a default icon when idle and a RED-DOT overlay variant while capturing,
//!   - a menu (Stop capture / Open AudioGraph / Quit) whose *Stop* item is
//!     enabled only while capturing,
//!   - a **content-free** tooltip that shows capture duration only — never
//!     transcript text, meeting titles, or any captured content (privacy
//!     constraint from the 2026-07-05 UX review, shortlist row 1/3).
//!
//! Capture state is owned frontend-side (the store's `isCapturing` spans
//! multiple sources), so the tray icon swap is driven by the frontend calling
//! the [`crate::commands::update_tray_capturing`] command whenever capture
//! transitions, and directly from the global-shortcut path once the store
//! round-trips. The tray never derives capture content — only the boolean
//! capturing flag + an already-formatted, content-free duration label.
//!
//! Runtime tray behavior needs a display, so it is verified manually (see the
//! PR body checklist). The headlessly-testable pieces — the red-dot overlay
//! selection and the tooltip formatting — are unit-tested below.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

/// Last-applied capturing state. The heavy tray work (icon decode/overlay +
/// menu rebuild) only needs to run on a capture *transition*; the per-second
/// duration ticks that keep the tooltip live must not rebuild the menu on every
/// call. This guard makes the common tick path a single cheap `set_tooltip`.
static LAST_CAPTURING: AtomicBool = AtomicBool::new(false);

/// Stable id for the app's single tray icon. Used to look the tray up again
/// from command handlers via [`AppHandle::tray_by_id`].
pub const TRAY_ID: &str = "audiograph-main-tray";

/// Menu item id: stop the active capture. Enabled only while capturing.
///
/// This string literal is identical to [`crate::events::TRAY_STOP_CAPTURE`],
/// but that is coincidental, not a shared constant: menu item ids and emitted
/// event names live in disjoint muda/Tauri namespaces (`event.id` here vs.
/// `app.emit`'s channel name below), so either value can change independently
/// without breaking the other.
const MENU_STOP: &str = "tray-stop-capture";
/// Menu item id: show + focus the main window.
const MENU_OPEN: &str = "tray-open";
/// Menu item id: quit the application (real exit, even while capturing).
const MENU_QUIT: &str = "tray-quit";

/// Idle tooltip — content-free. Never shows captured content.
const TOOLTIP_IDLE: &str = "AudioGraph — idle";

/// Overlay a solid red dot onto the bottom-right quadrant of an existing RGBA
/// icon, returning an owned recording-indicator variant.
///
/// The overlay is drawn programmatically (no second asset to ship) as a filled
/// circle of radius `~28%` of the icon size, centered in the bottom-right
/// quadrant, with a thin white ring for contrast against dark tray backgrounds.
/// Pixels are composited opaque so the dot reads at small tray sizes.
///
/// `rgba` must be `width * height * 4` bytes (row-major, RGBA8). Returns the
/// input unchanged (as owned) if the buffer length doesn't match so a
/// malformed source icon can never panic the tray build.
pub fn red_dot_overlay(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut out = rgba.to_vec();
    let (w, h) = (width as i64, height as i64);
    if w <= 0 || h <= 0 || out.len() != (w * h * 4) as usize {
        return out;
    }

    // Dot geometry: centered in the bottom-right quadrant.
    let min_dim = w.min(h);
    let radius = (min_dim * 28) / 100; // ~28% of the smaller dimension
    let ring = (radius / 6).max(1); // white contrast ring thickness
    let cx = w - radius - ring - (min_dim / 20);
    let cy = h - radius - ring - (min_dim / 20);
    let outer = radius + ring;

    for y in (cy - outer).max(0)..(cy + outer).min(h) {
        for x in (cx - outer).max(0)..(cx + outer).min(w) {
            let dx = x - cx;
            let dy = y - cy;
            let dist_sq = dx * dx + dy * dy;
            let idx = ((y * w + x) * 4) as usize;
            if dist_sq <= radius * radius {
                // Solid red core.
                out[idx] = 0xE1; // R
                out[idx + 1] = 0x1D; // G
                out[idx + 2] = 0x2E; // B
                out[idx + 3] = 0xFF; // A
            } else if dist_sq <= outer * outer {
                // White contrast ring.
                out[idx] = 0xFF;
                out[idx + 1] = 0xFF;
                out[idx + 2] = 0xFF;
                out[idx + 3] = 0xFF;
            }
        }
    }
    out
}

/// Format the CONTENT-FREE tray tooltip for the given capture state.
///
/// While idle → a fixed idle string. While capturing → "Recording · M:SS"
/// (or "H:MM:SS" past an hour) built solely from the elapsed-seconds count the
/// frontend already renders in the ControlBar. This NEVER includes transcript
/// text, note content, speaker labels, or meeting titles — only wall-clock
/// duration (UX-review privacy constraint).
pub fn tooltip_for(capturing: bool, elapsed_secs: Option<u64>) -> String {
    if !capturing {
        return TOOLTIP_IDLE.to_string();
    }
    match elapsed_secs {
        Some(secs) => format!("Recording · {}", format_duration(secs)),
        None => "Recording".to_string(),
    }
}

/// Format an elapsed-seconds count as `M:SS` (or `H:MM:SS` past one hour).
fn format_duration(total_secs: u64) -> String {
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    if hours > 0 {
        format!("{hours}:{mins:02}:{secs:02}")
    } else {
        format!("{mins}:{secs:02}")
    }
}

/// Build the tray menu. Extracted so the *Stop* item can be looked up by id
/// later for enable/disable, and so the menu construction is unit-constructible
/// in a headless test with a mock app handle.
pub fn build_menu<R: tauri::Runtime>(
    app: &AppHandle<R>,
    capturing: bool,
) -> tauri::Result<Menu<R>> {
    // Stop is enabled only while capturing.
    let stop = MenuItem::with_id(app, MENU_STOP, "Stop capture", capturing, None::<&str>)?;
    let open = MenuItem::with_id(app, MENU_OPEN, "Open AudioGraph", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    // Quit is a NORMAL menu item handled via `app.exit(0)` in the menu-event
    // handler, NOT `PredefinedMenuItem::quit` — muda documents the predefined
    // Quit as unsupported on Linux (our primary target), where it would render
    // but do nothing. The explicit handler is portable everywhere.
    let quit = MenuItem::with_id(app, MENU_QUIT, "Quit AudioGraph", true, None::<&str>)?;
    Menu::with_items(app, &[&stop, &open, &sep, &quit])
}

/// Show + focus the main window (used by tray left-click and the Open item).
fn show_main_window<R: tauri::Runtime>(app: &AppHandle<R>) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// Build the system tray during Tauri setup. Called once from `lib.rs`.
///
/// The base icon is the app's default window icon; the recording variant is
/// derived from it via [`red_dot_overlay`] and cached so the icon swap on
/// capture transitions is a cheap `set_icon` with no re-decode.
pub fn build_tray<R: tauri::Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let menu = build_menu(app, false)?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip(TOOLTIP_IDLE)
        .on_menu_event(|app, event| match event.id.as_ref() {
            MENU_STOP => {
                // Route Stop through the frontend store's stopCapture (same path
                // as the UI Stop button) by emitting an event the frontend
                // listens to — no parallel capture-stop logic in Rust. The event
                // name's single source of truth is `events::TRAY_STOP_CAPTURE`.
                use tauri::Emitter;
                let _ = app.emit(crate::events::TRAY_STOP_CAPTURE, ());
            }
            MENU_OPEN => show_main_window(app),
            // Real exit, even while capturing. A normal item + explicit
            // `app.exit(0)` because muda's predefined Quit is a no-op on Linux.
            MENU_QUIT => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // Left-click (press+release) shows + focuses the window.
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });

    // Seed the idle icon from the app's default window icon.
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    builder.build(app)?;
    Ok(())
}

/// Apply a capture-state change to the tray: swap the icon (red-dot while
/// capturing, default when idle), refresh the CONTENT-FREE tooltip, and toggle
/// the *Stop capture* menu item's enabled state.
///
/// Idempotent and best-effort: every step logs-and-continues on error so a
/// tray hiccup can never break the capture command that drives it.
pub fn apply_capture_state<R: tauri::Runtime>(
    app: &AppHandle<R>,
    capturing: bool,
    elapsed_secs: Option<u64>,
) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        // No tray (e.g. headless / unsupported platform) — nothing to update.
        return;
    };

    // Only rebuild the icon + menu on a capture *transition*; ticks that merely
    // advance the duration counter fall through to the cheap tooltip update.
    let transitioned = LAST_CAPTURING.swap(capturing, Ordering::SeqCst) != capturing;

    if transitioned {
        // Icon swap: derive the red-dot variant from the default window icon.
        if let Some(base) = app.default_window_icon() {
            let icon = if capturing {
                let rgba = red_dot_overlay(base.rgba(), base.width(), base.height());
                Image::new_owned(rgba, base.width(), base.height())
            } else {
                base.clone().to_owned()
            };
            if let Err(e) = tray.set_icon(Some(icon)) {
                log::warn!("tray: failed to set icon (capturing={capturing}): {e}");
            }
        }

        // Toggle Stop-capture menu item enabled state.
        match build_menu(app, capturing) {
            Ok(menu) => {
                if let Err(e) = tray.set_menu(Some(menu)) {
                    log::warn!("tray: failed to refresh menu: {e}");
                }
            }
            Err(e) => log::warn!("tray: failed to rebuild menu: {e}"),
        }
    }

    // Content-free tooltip — refreshed on every call so the duration stays live.
    if let Err(e) = tray.set_tooltip(Some(tooltip_for(capturing, elapsed_secs))) {
        log::warn!("tray: failed to set tooltip: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltip_is_idle_when_not_capturing() {
        assert_eq!(tooltip_for(false, Some(42)), TOOLTIP_IDLE);
        assert_eq!(tooltip_for(false, None), TOOLTIP_IDLE);
    }

    #[test]
    fn tooltip_shows_content_free_duration_when_capturing() {
        assert_eq!(tooltip_for(true, Some(0)), "Recording · 0:00");
        assert_eq!(tooltip_for(true, Some(5)), "Recording · 0:05");
        assert_eq!(tooltip_for(true, Some(65)), "Recording · 1:05");
        assert_eq!(tooltip_for(true, Some(3661)), "Recording · 1:01:01");
    }

    #[test]
    fn tooltip_capturing_without_elapsed_is_bare_recording() {
        assert_eq!(tooltip_for(true, None), "Recording");
    }

    #[test]
    fn tooltip_never_contains_content_markers() {
        // Defensive: the tooltip must be derivable ONLY from (bool, seconds);
        // there is no code path that can inject transcript/title content.
        let t = tooltip_for(true, Some(120));
        assert!(t.starts_with("Recording · "));
        // Only digits, a colon, and the fixed "Recording · " prefix — never any
        // captured content. (The middot is non-ASCII; assert the numeric tail is.)
        let tail = t.trim_start_matches("Recording · ");
        assert!(tail.chars().all(|c| c.is_ascii_digit() || c == ':'));
    }

    #[test]
    fn format_duration_rolls_minutes_and_hours() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(59), "0:59");
        assert_eq!(format_duration(60), "1:00");
        assert_eq!(format_duration(600), "10:00");
        assert_eq!(format_duration(3600), "1:00:00");
        assert_eq!(format_duration(3725), "1:02:05");
    }

    #[test]
    fn red_dot_overlay_paints_bottom_right_and_preserves_len() {
        // 16x16 fully-transparent base.
        let w = 16u32;
        let h = 16u32;
        let base = vec![0u8; (w * h * 4) as usize];
        let out = red_dot_overlay(&base, w, h);
        assert_eq!(out.len(), base.len(), "overlay preserves buffer length");

        // The top-left pixel is far from the bottom-right dot → untouched.
        assert_eq!(&out[0..4], &[0, 0, 0, 0], "top-left stays transparent");

        // Some pixel in the bottom-right quadrant is now opaque red.
        let has_red = out
            .chunks_exact(4)
            .any(|px| px[0] == 0xE1 && px[1] == 0x1D && px[2] == 0x2E && px[3] == 0xFF);
        assert!(has_red, "overlay paints an opaque red dot");

        // And a white contrast ring pixel exists.
        let has_white_ring = out
            .chunks_exact(4)
            .any(|px| px[0] == 0xFF && px[1] == 0xFF && px[2] == 0xFF && px[3] == 0xFF);
        assert!(has_white_ring, "overlay paints a white contrast ring");
    }

    #[test]
    fn red_dot_overlay_rejects_mismatched_buffer_without_panicking() {
        // Buffer length inconsistent with declared dims → returned unchanged.
        let bad = vec![1u8, 2, 3, 4, 5];
        let out = red_dot_overlay(&bad, 16, 16);
        assert_eq!(out, bad, "mismatched buffer is returned unchanged");
    }

    #[test]
    fn red_dot_overlay_handles_zero_dims() {
        let out = red_dot_overlay(&[], 0, 0);
        assert!(out.is_empty());
    }
}
