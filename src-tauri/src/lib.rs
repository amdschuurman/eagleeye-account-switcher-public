//! EagleEye Account Switcher: native engine. Modules follow the DESIGN.md contract: `paths` (roots),
//! `store` (accounts.json + settings.json), `vault` (DPAPI token files), `config` (Battle.net.config
//! editing), `registry` (UnifiedAuth), `procs` (process control), `logwatch` (the sign-in line),
//! `engine` (the switch/add/save/import state machine), `commands` (the Tauri surface), `tray`
//! (tray icon, close-to-tray, close after switch) and `updater` (optional updates, offered, never forced).

mod atomic;
mod avatars;
mod battletag;
mod clock;
mod commands;
mod config;
mod engine;
mod icons;
mod logwatch;
mod overfast;
mod paths;
mod procs;
mod ranks;
mod registry;
mod store;
mod tcno;
mod tray;
mod updater;
mod vault;

use engine::Backend;
use std::sync::Arc;

/// Build the backend with the real platform implementations (Windows), or dev stubs elsewhere.
fn build_backend() -> Result<Backend, String> {
    let paths = paths::Paths::discover()?;
    #[cfg(windows)]
    {
        Ok(Backend::new(
            paths,
            Box::new(registry::WinRegistry),
            Box::new(procs::WinProcs),
            Box::new(clock::SystemClock::default()),
            Box::new(vault::DpapiProtector),
        ))
    }
    #[cfg(not(windows))]
    {
        Ok(Backend::new(
            paths,
            Box::new(registry::DevRegistry::default()),
            Box::new(procs::DevProcs),
            Box::new(clock::SystemClock::default()),
            Box::new(vault::DevProtector),
        ))
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let backend = match build_backend() {
        Ok(b) => Arc::new(b),
        Err(e) => {
            eprintln!("EagleEye Account Switcher could not start: {e}");
            return;
        }
    };

    let builder = tauri::Builder::default();

    // One running instance: a second launch focuses the existing window instead of opening another.
    #[cfg(windows)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| tray::show_main(app)));

    builder
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(backend)
        .setup(|app| {
            use tauri::Manager as _;
            let updates_on = updater::configured(app.config().plugins.0.get("updater"));
            app.manage(Arc::new(updater::Updater::new(&app.package_info().version.to_string(), updates_on)));
            tray::create(app.handle())?;
            updater::start(app.handle());
            Ok(())
        })
        .on_window_event(tray::on_window_event)
        .invoke_handler(tauri::generate_handler![
            commands::list_accounts,
            commands::current_account,
            commands::switch_account,
            commands::add_account,
            commands::cancel_add,
            commands::save_current,
            commands::import_tcno,
            commands::tcno_available,
            commands::rename_account,
            commands::remove_account,
            commands::reorder_accounts,
            commands::get_settings,
            commands::set_settings,
            commands::get_ranks,
            commands::get_all_ranks,
            commands::refresh_ranks,
            commands::set_rank_override,
            commands::set_category,
            commands::icon_dir,
            commands::sort_by_rank,
            commands::update_state,
            commands::check_update,
            commands::install_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running EagleEye Account Switcher");
}
