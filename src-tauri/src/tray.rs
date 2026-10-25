//! The tray icon and window behaviour around a switch. The tray is always present: left click shows
//! the window; the menu lists every saved account (click = switch directly), then Show and Quit. The
//! menu is rebuilt when the pointer enters the icon, so it always reflects accounts.json and the
//! signed-in account. Settings decide two behaviours:
//! - `minimizeToTray`: the window's close button hides the window to the tray instead of quitting.
//! - `closeAfterSwitch`: after a successful switch the window goes away: hidden to the tray when
//!   `minimizeToTray` is on, otherwise the app exits (with no tray to come back from, a hidden window
//!   would be unreachable).

use crate::engine::{Backend, Emitter, SwitchResult};
use std::sync::Arc;
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime, Window, WindowEvent};

const TRAY_ID: &str = "main";
const SHOW_ID: &str = "show";
const QUIT_ID: &str = "quit";
const SWITCH_PREFIX: &str = "switch:";

/// A menu id to the account code it switches to, if it is a switch item. Pure, unit-tested.
fn switch_code(menu_id: &str) -> Option<&str> {
    menu_id.strip_prefix(SWITCH_PREFIX).filter(|c| !c.is_empty())
}

/// The menu label for one account.
fn account_label(label: &str, signed_in: bool, needs_login: bool) -> String {
    match (signed_in, needs_login) {
        (true, _) => format!("{label} (signed in)"),
        (false, true) => format!("{label} (log in once)"),
        _ => label.to_string(),
    }
}

/// What the window does after a switch: `None` = stay, `Some(true)` = hide, `Some(false)` = exit.
fn after_switch_action(ok: bool, close_after_switch: bool, minimize_to_tray: bool) -> Option<bool> {
    (ok && close_after_switch).then_some(minimize_to_tray)
}

/// Emits switch progress started from the tray to the window, like the UI's own switches.
struct TrayEmitter<R: Runtime>(AppHandle<R>);
impl<R: Runtime> Emitter for TrayEmitter<R> {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        use tauri::Emitter as _;
        let _ = self.0.emit(event, payload);
    }
}

pub fn show_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
    crate::presence::set_visible(app, true);
}

/// Called after every switch (UI or tray) with its result.
pub fn after_switch<R: Runtime>(app: &AppHandle<R>, result: &SwitchResult) {
    let settings = app.state::<Arc<Backend>>().get_settings();
    match after_switch_action(result.status == "done", settings.close_after_switch, settings.minimize_to_tray) {
        Some(true) => {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.hide();
            }
            crate::presence::set_visible(app, false);
        }
        Some(false) => app.exit(0),
        None => {}
    }
}

fn build_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let be = app.state::<Arc<Backend>>();
    let accounts = be.list_accounts().unwrap_or_default();
    let current = be.current_account().ok().flatten();
    let menu = Menu::new(app)?;
    for a in &accounts {
        let text = account_label(
            &a.label,
            current.as_deref() == Some(a.code.as_str()),
            a.status == crate::store::Status::NeedsLogin,
        );
        menu.append(&MenuItem::with_id(app, format!("{SWITCH_PREFIX}{}", a.code), text, true, None::<&str>)?)?;
    }
    if !accounts.is_empty() {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }
    menu.append(&MenuItem::with_id(app, SHOW_ID, "Show", true, None::<&str>)?)?;
    menu.append(&MenuItem::with_id(app, QUIT_ID, "Quit", true, None::<&str>)?)?;
    Ok(menu)
}

fn refresh_menu<R: Runtime>(app: &AppHandle<R>) {
    if let (Some(tray), Ok(menu)) = (app.tray_by_id(TRAY_ID), build_menu(app)) {
        let _ = tray.set_menu(Some(menu));
    }
}

fn on_menu<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    let id = event.id().as_ref();
    match id {
        SHOW_ID => show_main(app),
        QUIT_ID => app.exit(0),
        _ => {
            let Some(code) = switch_code(id).map(str::to_string) else { return };
            let app = app.clone();
            // A switch blocks for seconds: run it off the event loop. The engine's busy flag ignores
            // a second click while one runs.
            std::thread::spawn(move || {
                let be = app.state::<Arc<Backend>>().inner().clone();
                let presence = crate::presence::busy(&app, crate::presence::Doing::Switching);
                let result = be.switch_account(&code, &TrayEmitter(app.clone()));
                drop(presence);
                after_switch(&app, &result);
                if result.status == "done" {
                    crate::battletag::after_sign_in(&app, &code);
                    crate::commands::recapture_later(be.clone(), code.clone());
                }
                refresh_menu(&app);
            });
        }
    }
}

fn on_tray<R: Runtime>(tray: &TrayIcon<R>, event: TrayIconEvent) {
    match event {
        TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } => {
            show_main(tray.app_handle())
        }
        // Rebuild just before the user can open the menu, so it lists the current accounts.
        TrayIconEvent::Enter { .. } => refresh_menu(tray.app_handle()),
        _ => {}
    }
}

/// Create the tray icon (call from `setup`).
pub fn create<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("EagleEye Account Switcher")
        .menu(&build_menu(app)?)
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu)
        .on_tray_icon_event(on_tray);
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

/// Window close button: hide to the tray when `minimizeToTray` is on, else close as normal.
pub fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    if let WindowEvent::CloseRequested { api, .. } = event {
        if window.label() == "main" && window.state::<Arc<Backend>>().get_settings().minimize_to_tray {
            api.prevent_close();
            let _ = window.hide();
            crate::presence::set_visible(window.app_handle(), false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_map_to_codes() {
        assert_eq!(switch_code("switch:aaaa0001"), Some("aaaa0001"));
        assert_eq!(switch_code("switch:"), None);
        assert_eq!(switch_code(SHOW_ID), None);
        assert_eq!(switch_code(QUIT_ID), None);
    }

    #[test]
    fn account_labels_mark_state() {
        assert_eq!(account_label("Alice", true, false), "Alice (signed in)");
        assert_eq!(account_label("Smurf", false, true), "Smurf (log in once)");
        assert_eq!(account_label("Main", false, false), "Main");
    }

    #[test]
    fn after_switch_hides_exits_or_stays() {
        assert_eq!(after_switch_action(true, true, true), Some(true), "hide to tray");
        assert_eq!(after_switch_action(true, true, false), Some(false), "exit, no tray to return from");
        assert_eq!(after_switch_action(true, false, true), None, "setting off: stay");
        assert_eq!(after_switch_action(false, true, true), None, "failed switch: stay");
    }
}
