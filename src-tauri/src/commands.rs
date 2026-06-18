//! Tauri-команды и состояние приложения (#8/#9/#10).
//!
//! Управляемое состояние — `Mutex<Store>`. Команды для фронта окна настроек,
//! построение и обновление меню в баре, запуск переключения аккаунта.

use crate::store::{Account, SecretKind, Store, UsageSnapshot};
use crate::{capture, swap};
use serde::Serialize;
use std::sync::Mutex;
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder},
    AppHandle, Emitter, Manager,
};

/// Состояние приложения.
pub struct AppState(pub Mutex<Store>);

/// Представление аккаунта для фронта — без cookie-блобов.
#[derive(Debug, Clone, Serialize)]
pub struct AccountView {
    pub id: String,
    pub display_name: String,
    pub email: String,
    pub email_url: String,
    pub has_cookies: bool,
    pub session_expires_utc: Option<i64>,
    pub usage: Option<UsageSnapshot>,
    pub is_active: bool,
}

fn to_view(a: &Account, active_id: Option<&str>) -> AccountView {
    let sk = a.cookies.iter().find(|c| c.name == "sessionKey");
    AccountView {
        id: a.id.clone(),
        display_name: a.display_name.clone(),
        email: a.email.clone(),
        email_url: a.email_url.clone(),
        has_cookies: sk.is_some(),
        session_expires_utc: sk.map(|c| c.expires_utc),
        usage: a.usage.clone(),
        is_active: active_id == Some(a.id.as_str()),
    }
}

// ───────────────────────── Команды CRUD ─────────────────────────

#[tauri::command]
pub fn list_accounts(state: tauri::State<AppState>) -> Vec<AccountView> {
    let store = state.0.lock().unwrap();
    let active = store.active_id.clone();
    store
        .accounts
        .iter()
        .map(|a| to_view(a, active.as_deref()))
        .collect()
}

/// Параметры добавления/редактирования аккаунта из формы.
#[derive(serde::Deserialize)]
pub struct AccountInput {
    pub id: Option<String>,
    pub display_name: String,
    pub email: String,
    pub email_url: String,
    pub password: Option<String>,
    pub email_password: Option<String>,
}

#[tauri::command]
pub fn save_account(app: AppHandle, state: tauri::State<AppState>, input: AccountInput) -> Result<String, String> {
    let id = {
        let mut store = state.0.lock().unwrap();
        let id = match &input.id {
            Some(id) => {
                // редактирование: сохранить cookie и usage существующего
                let existing = store.get(id).cloned();
                let mut acc = existing.ok_or_else(|| "аккаунт не найден".to_string())?;
                acc.display_name = input.display_name.clone();
                acc.email = input.email.clone();
                acc.email_url = input.email_url.clone();
                store.update(acc);
                id.clone()
            }
            None => {
                let acc = Account::new(
                    input.display_name.clone(),
                    input.email.clone(),
                    input.email_url.clone(),
                );
                store.add(acc)
            }
        };
        store.save().map_err(|e| e.to_string())?;
        id
    };

    // секреты — в Keychain (только если переданы непустыми)
    if let Some(p) = input.password.as_deref().filter(|s| !s.is_empty()) {
        crate::store::set_secret(&id, SecretKind::Password, p).map_err(|e| e.to_string())?;
    }
    if let Some(p) = input.email_password.as_deref().filter(|s| !s.is_empty()) {
        crate::store::set_secret(&id, SecretKind::EmailPassword, p).map_err(|e| e.to_string())?;
    }

    refresh_tray(&app);
    Ok(id)
}

#[tauri::command]
pub fn delete_account(app: AppHandle, state: tauri::State<AppState>, id: String) -> Result<(), String> {
    {
        let mut store = state.0.lock().unwrap();
        store.delete(&id);
        store.save().map_err(|e| e.to_string())?;
    }
    refresh_tray(&app);
    Ok(())
}

/// Секреты аккаунта для предзаполнения формы редактирования.
#[derive(Serialize)]
pub struct AccountSecrets {
    pub password: Option<String>,
    pub email_password: Option<String>,
}

#[tauri::command]
pub fn get_account_secrets(id: String) -> AccountSecrets {
    AccountSecrets {
        password: crate::store::get_secret(&id, SecretKind::Password).unwrap_or(None),
        email_password: crate::store::get_secret(&id, SecretKind::EmailPassword).unwrap_or(None),
    }
}

/// Захватить текущую сессию Claude и привязать к аккаунту (создав при отсутствии id).
#[tauri::command]
pub fn capture_account(
    app: AppHandle,
    state: tauri::State<AppState>,
    id: Option<String>,
) -> Result<String, String> {
    let cookies = capture::capture_cookies().map_err(|e| e.to_string())?;
    let id = {
        let mut store = state.0.lock().unwrap();
        let id = match id {
            Some(id) => id,
            None => return Err("сначала сохраните аккаунт, потом захватывайте cookie".into()),
        };
        let acc = store.get_mut(&id).ok_or_else(|| "аккаунт не найден".to_string())?;
        acc.cookies = cookies;
        // тот, чью сессию захватили, сейчас и активен в Claude
        store.set_active(&id);
        store.save().map_err(|e| e.to_string())?;
        id
    };
    refresh_tray(&app);
    Ok(id)
}

// ───────────────────────── Переключение ─────────────────────────

/// Запустить переключение на аккаунт в фоне (свап завершает Claude).
#[tauri::command]
pub fn switch_account(app: AppHandle, id: String) {
    trigger_switch(app, id);
}

/// Внутренний запуск свапа (используется и из меню tray, и из команды).
pub fn trigger_switch(app: AppHandle, id: String) {
    let app2 = app.clone();
    std::thread::spawn(move || {
        let state = app2.state::<AppState>();
        let cookies = {
            let store = state.0.lock().unwrap();
            match store.active_id.as_deref() == Some(id.as_str()) {
                true => {
                    let _ = app2.emit("switch-progress", "Уже активен");
                    return;
                }
                false => store.get(&id).map(|a| a.cookies.clone()),
            }
        };
        let Some(cookies) = cookies else {
            let _ = app2.emit("switch-error", "аккаунт не найден");
            return;
        };
        if cookies.is_empty() {
            let _ = app2.emit("switch-error", "у аккаунта нет захваченных cookie");
            return;
        }

        let res = swap::switch_to(&cookies, |stage| {
            let _ = app2.emit("switch-progress", stage.message());
        });

        match res {
            Ok(()) => {
                {
                    let mut store = state.0.lock().unwrap();
                    store.set_active(&id);
                    let _ = store.save();
                }
                refresh_tray(&app2);
                let _ = app2.emit("switch-done", id);
            }
            Err(e) => {
                let _ = app2.emit("switch-error", e.to_string());
            }
        }
    });
}

// ───────────────────────── Меню в баре ─────────────────────────

/// Построить меню tray из текущего состояния.
pub fn build_menu(app: &AppHandle, store: &Store) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    let mut switch = SubmenuBuilder::new(app, "Переключиться");
    if store.accounts.is_empty() {
        switch = switch.item(
            &MenuItemBuilder::with_id("noop", "Нет аккаунтов")
                .enabled(false)
                .build(app)?,
        );
    } else {
        for a in &store.accounts {
            let active = store.active_id.as_deref() == Some(a.id.as_str());
            let mark = if active { "✓ " } else { "   " };
            let label = format!("{mark}{}", a.display_name);
            switch = switch.item(&MenuItemBuilder::with_id(format!("switch:{}", a.id), label).build(app)?);
        }
    }
    let switch = switch.build()?;

    let settings = MenuItemBuilder::with_id("settings", "Настройки…").build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Выйти").build(app)?;

    MenuBuilder::new(app)
        .item(&switch)
        .separator()
        .item(&settings)
        .item(&quit)
        .build()
}

/// Заголовок tray: имя активного аккаунта (или «cc»).
fn tray_title(store: &Store) -> String {
    store
        .active()
        .map(|a| a.display_name.clone())
        .unwrap_or_else(|| "cc".to_string())
}

/// Перестроить меню и заголовок tray из состояния.
pub fn refresh_tray(app: &AppHandle) {
    let state = app.state::<AppState>();
    let store = state.0.lock().unwrap();
    if let Ok(menu) = build_menu(app, &store) {
        if let Some(tray) = app.tray_by_id("main") {
            let _ = tray.set_menu(Some(menu));
            #[cfg(target_os = "macos")]
            let _ = tray.set_title(Some(tray_title(&store)));
        }
    }
}
