//! Tauri-команды и состояние приложения (#8/#9/#10).
//!
//! Управляемое состояние — `Mutex<Store>`. Команды для фронта окна настроек,
//! построение и обновление меню в баре, запуск переключения аккаунта.

use crate::store::{Account, Store, UsageSnapshot};
use crate::{capture, swap};
use serde::Serialize;
use std::sync::Mutex;
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    AppHandle, Emitter, Manager,
};

/// Состояние приложения.
pub struct AppState(pub Mutex<Store>);

/// Запрос на ввод времени ресета вокруг переключения (окно-промпт).
#[derive(Debug, Clone, Serialize)]
pub struct SwitchPrompt {
    /// На какой аккаунт переключаемся.
    pub target_id: String,
    pub target_name: String,
    /// Уходящий (активный) аккаунт, если он есть и отличается от целевого.
    pub outgoing_id: Option<String>,
    pub outgoing_name: Option<String>,
    /// Целевой аккаунт уже активен — свап не нужен, только задать его ресет.
    pub already_active: bool,
}

/// Очередь из одного запроса на ввод времени ресета (читает окно-промпт).
pub struct PendingPrompt(pub Mutex<Option<SwitchPrompt>>);

/// Представление аккаунта для фронта — без cookie-блобов.
#[derive(Debug, Clone, Serialize)]
pub struct AccountView {
    pub id: String,
    pub display_name: String,
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
                store.update(acc);
                id.clone()
            }
            None => store.add(Account::new(input.display_name.clone())),
        };
        store.save().map_err(|e| e.to_string())?;
        id
    };

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

// ───────────────────── Время ресета (ручной ввод) ─────────────────────

/// Сохранить введённое человеком время ресета сессии аккаунта.
/// `reset_at` — unix-секунды (фронт считает ближайшую будущую метку),
/// `label` — подпись «HH:MM» для показа.
#[tauri::command]
pub fn set_reset_time(
    app: AppHandle,
    state: tauri::State<AppState>,
    id: String,
    reset_at: i64,
    label: String,
) -> Result<(), String> {
    {
        let mut store = state.0.lock().unwrap();
        let acc = store.get_mut(&id).ok_or("аккаунт не найден")?;
        let mut usage = acc.usage.clone().unwrap_or_default();
        usage.reset_at = Some(reset_at);
        usage.reset_label = Some(label);
        usage.captured_at = crate::store::now_unix();
        acc.usage = Some(usage);
        store.save().map_err(|e| e.to_string())?;
    }
    refresh_tray(&app);
    Ok(())
}

/// Очистить заданное время ресета аккаунта.
#[tauri::command]
pub fn clear_reset_time(
    app: AppHandle,
    state: tauri::State<AppState>,
    id: String,
) -> Result<(), String> {
    {
        let mut store = state.0.lock().unwrap();
        let acc = store.get_mut(&id).ok_or("аккаунт не найден")?;
        if let Some(u) = acc.usage.as_mut() {
            u.reset_at = None;
            u.reset_label = None;
        }
        store.save().map_err(|e| e.to_string())?;
    }
    refresh_tray(&app);
    Ok(())
}

/// Прочитать (не очищая) запрос на ввод времени ресета для окна-промпта.
#[tauri::command]
pub fn get_pending_prompt(state: tauri::State<PendingPrompt>) -> Option<SwitchPrompt> {
    state.0.lock().unwrap().clone()
}

// ───────────────────────── Переключение ─────────────────────────

/// Запустить переключение на аккаунт в фоне (свап завершает Claude).
#[tauri::command]
pub fn switch_account(app: AppHandle, id: String) {
    trigger_switch(app, id);
}

/// Открыть окно-промпт ввода времени ресета вокруг переключения на `target_id`.
/// Вызывается из меню tray вместо прямого свапа: само переключение и сохранение
/// времени ресета оркеструет окно-промпт (см. фронт).
pub fn open_switch_prompt(app: AppHandle, target_id: String) {
    {
        let state = app.state::<AppState>();
        let store = state.0.lock().unwrap();
        let Some(target) = store.get(&target_id) else {
            return;
        };
        let already_active = store.active_id.as_deref() == Some(target_id.as_str());
        let (outgoing_id, outgoing_name) = if already_active {
            (None, None)
        } else {
            match store.active() {
                Some(a) => (Some(a.id.clone()), Some(a.display_name.clone())),
                None => (None, None),
            }
        };
        let prompt = SwitchPrompt {
            target_id: target_id.clone(),
            target_name: target.display_name.clone(),
            outgoing_id,
            outgoing_name,
            already_active,
        };
        let pending = app.state::<PendingPrompt>();
        *pending.0.lock().unwrap() = Some(prompt);
    }
    show_prompt_window(&app);
}

/// Показать (создав при необходимости) окно-промпт и попросить его перечитать запрос.
fn show_prompt_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("prompt") {
        let _ = win.show();
        let _ = win.set_focus();
        let _ = app.emit("prompt-show", ());
    } else {
        let _ = tauri::WebviewWindowBuilder::new(
            app,
            "prompt",
            tauri::WebviewUrl::App("prompt.html".into()),
        )
        .title("cc-switcher — переключение")
        .inner_size(380.0, 300.0)
        .min_inner_size(380.0, 300.0)
        .resizable(false)
        .always_on_top(true)
        .center()
        .build();
    }
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

/// Построить меню tray: аккаунты сразу в корне (без подменю), отсортированы
/// по времени сброса, формат «hh:mm – имя» (✓ у активного). Ниже — Настройки/Выйти.
pub fn build_menu(app: &AppHandle, store: &Store) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    let mut b = MenuBuilder::new(app);

    if store.accounts.is_empty() {
        b = b.item(
            &MenuItemBuilder::with_id("noop", "Нет аккаунтов")
                .enabled(false)
                .build(app)?,
        );
    } else {
        // сортировка по времени сброса (заданные раньше → выше, без времени → в конец)
        let mut accs: Vec<&Account> = store.accounts.iter().collect();
        accs.sort_by(|x, y| {
            let kx = x.usage.as_ref().and_then(|u| u.reset_at);
            let ky = y.usage.as_ref().and_then(|u| u.reset_at);
            match (kx, ky) {
                (Some(a), Some(b)) => a.cmp(&b),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => x.display_name.cmp(&y.display_name),
            }
        });
        for a in accs {
            let active = store.active_id.as_deref() == Some(a.id.as_str());
            let mark = if active { "✓ " } else { "" };
            let time = a.usage.as_ref().and_then(|u| u.reset_label.clone());
            let label = match time {
                Some(t) => format!("{mark}{t} – {}", a.display_name),
                None => format!("{mark}{}", a.display_name),
            };
            b = b.item(&MenuItemBuilder::with_id(format!("switch:{}", a.id), label).build(app)?);
        }
    }

    let settings = MenuItemBuilder::with_id("settings", "Настройки…").build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Выйти").build(app)?;

    b.separator().item(&settings).item(&quit).build()
}

/// Подпись времени ресета аккаунта, если оно задано и ещё не наступило.
fn reset_label(a: &Account) -> Option<String> {
    let u = a.usage.as_ref()?;
    let at = u.reset_at?;
    let label = u.reset_label.as_ref()?;
    (at > crate::store::now_unix()).then(|| label.clone())
}

/// Заголовок tray: имя активного аккаунта и его время ресета (или «cc»).
fn tray_title(store: &Store) -> String {
    match store.active() {
        Some(a) => match reset_label(a) {
            Some(l) => format!("{} · {}", a.display_name, l),
            None => a.display_name.clone(),
        },
        None => "cc".to_string(),
    }
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
