// Публичный API задействуют следующие задачи (#6 capture, #9 меню, #10 настройки).
#[allow(dead_code)]
mod capture;
#[allow(dead_code)]
mod cookies;
#[allow(dead_code)]
mod store;
#[allow(dead_code)]
mod swap;

use tauri::{
    menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder},
    tray::TrayIconBuilder,
    Manager,
};

/// Показать (создать при необходимости) окно настроек.
fn show_settings(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("settings") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

/// Построить иконку в меню-баре с меню.
fn build_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    // Подменю «Переключиться» — пока заглушка, аккаунты добавит #9/#5.
    let no_accounts = MenuItemBuilder::with_id("no_accounts", "Нет аккаунтов")
        .enabled(false)
        .build(app)?;
    let switch = SubmenuBuilder::new(app, "Переключиться")
        .item(&no_accounts)
        .build()?;

    let settings = MenuItemBuilder::with_id("settings", "Настройки…").build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Выйти").build(app)?;

    let menu = MenuBuilder::new(app)
        .item(&switch)
        .separator()
        .item(&settings)
        .item(&quit)
        .build()?;

    let tray = TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().unwrap().clone())
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "settings" => show_settings(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;

    // Текст рядом с иконкой — заглушка, остаток сессии подставит #7.
    #[cfg(target_os = "macos")]
    let _ = tray.set_title(Some("cc"));

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
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
