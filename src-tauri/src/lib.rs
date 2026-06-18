// Часть публичного API задействуют следующие задачи.
#[allow(dead_code)]
mod capture;
mod commands;
#[allow(dead_code)]
mod cookies;
#[allow(dead_code)]
mod store;
#[allow(dead_code)]
mod swap;

use commands::AppState;
use std::sync::Mutex;
use store::Store;
use tauri::{tray::TrayIconBuilder, Manager};

/// Показать (создать при необходимости) окно настроек.
fn show_settings(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("settings") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

/// Построить иконку в меню-баре. Меню строится из состояния и обновляется
/// командами через [`commands::refresh_tray`].
fn build_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let menu = {
        let state = app.state::<AppState>();
        let store = state.0.lock().unwrap();
        commands::build_menu(app, &store)?
    };

    let tray = TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().unwrap().clone())
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            match id {
                "settings" => show_settings(app),
                "quit" => app.exit(0),
                other if other.starts_with("switch:") => {
                    let account_id = other.trim_start_matches("switch:").to_string();
                    commands::trigger_switch(app.clone(), account_id);
                }
                _ => {}
            }
        })
        .build(app)?;

    #[cfg(target_os = "macos")]
    {
        let state = app.state::<AppState>();
        let store = state.0.lock().unwrap();
        let title = store
            .active()
            .map(|a| a.display_name.clone())
            .unwrap_or_else(|| "cc".to_string());
        let _ = tray.set_title(Some(title));
    }

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let store = Store::load().unwrap_or_default();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState(Mutex::new(store)))
        .invoke_handler(tauri::generate_handler![
            commands::list_accounts,
            commands::save_account,
            commands::delete_account,
            commands::get_account_secrets,
            commands::capture_account,
            commands::switch_account,
        ])
        .setup(|app| {
            // Жить только в меню-баре: убрать иконку из Dock.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            build_tray(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Закрытие окна настроек прячет его, а не завершает приложение.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "settings" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
