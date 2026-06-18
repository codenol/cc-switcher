//! Хранилище аккаунтов (#5): модель, CRUD, снимки usage.
//!
//! Данные (аккаунты, cookie-блобы, снимок usage) — в JSON
//! `~/Library/Application Support/cc-switcher/accounts.json`.
//! Cookie лежат как зашифрованные блобы (см. [`crate::cookies`]).
//! Логины/пароли не хранятся: переключение работает только на cookie-swap.

use crate::cookies::SessionCookie;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Снимок остатка сессии аккаунта (момент последнего выхода/опроса).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UsageSnapshot {
    /// Остаток в процентах (0..100), если известен.
    pub percent_remaining: Option<f64>,
    /// Время сброса лимита, unix-секунды.
    pub reset_at: Option<i64>,
    /// Подпись времени ресета как ввёл человек, например «20:30» (для показа).
    #[serde(default)]
    pub reset_label: Option<String>,
    /// Когда снят снимок, unix-секунды.
    pub captured_at: i64,
}

/// Аккаунт: ярлык + захваченная сессия + время ресета.
/// Переключение работает только на cookie-swap; логины/пароли не нужны.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub display_name: String,
    /// Зашифрованные cookie сессии (блобы v10).
    #[serde(default)]
    pub cookies: Vec<SessionCookie>,
    /// Последний известный снимок usage (время ресета).
    #[serde(default)]
    pub usage: Option<UsageSnapshot>,
}

impl Account {
    pub fn new(display_name: String) -> Self {
        Account {
            id: uuid::Uuid::new_v4().to_string(),
            display_name,
            cookies: Vec::new(),
            usage: None,
        }
    }
}

/// Всё состояние приложения, сериализуемое на диск.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Store {
    pub accounts: Vec<Account>,
    /// id активного аккаунта (под которым сейчас залогинен Claude).
    pub active_id: Option<String>,
}

/// Текущее время в unix-секундах.
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Каталог данных приложения (переопределяется `CC_SWITCHER_DIR` для тестов).
pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CC_SWITCHER_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join("Library/Application Support/cc-switcher")
}

fn store_path() -> PathBuf {
    data_dir().join("accounts.json")
}

// ───────────────────────── Загрузка / сохранение ─────────────────────────

impl Store {
    /// Загрузить состояние с диска (или пустое, если файла нет).
    pub fn load() -> Result<Store> {
        let path = store_path();
        if !path.exists() {
            return Ok(Store::default());
        }
        let data = std::fs::read(&path)
            .with_context(|| format!("не удалось прочитать {}", path.display()))?;
        let store = serde_json::from_slice(&data).context("повреждён accounts.json")?;
        Ok(store)
    }

    /// Атомарно сохранить состояние на диск.
    pub fn save(&self) -> Result<()> {
        let dir = data_dir();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("не удалось создать {}", dir.display()))?;
        let path = store_path();
        let tmp = path.with_extension("json.tmp");
        let data = serde_json::to_vec_pretty(self)?;
        std::fs::write(&tmp, data)
            .with_context(|| format!("не удалось записать {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).context("не удалось зафиксировать accounts.json")?;
        Ok(())
    }

    // ───────────────────────── CRUD ─────────────────────────

    pub fn get(&self, id: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Account> {
        self.accounts.iter_mut().find(|a| a.id == id)
    }

    /// Добавить аккаунт, вернуть его id.
    pub fn add(&mut self, account: Account) -> String {
        let id = account.id.clone();
        self.accounts.push(account);
        id
    }

    /// Обновить существующий аккаунт (по id). Возвращает false, если не найден.
    pub fn update(&mut self, account: Account) -> bool {
        if let Some(slot) = self.accounts.iter_mut().find(|a| a.id == account.id) {
            *slot = account;
            true
        } else {
            false
        }
    }

    /// Удалить аккаунт.
    pub fn delete(&mut self, id: &str) -> bool {
        let before = self.accounts.len();
        self.accounts.retain(|a| a.id != id);
        if self.active_id.as_deref() == Some(id) {
            self.active_id = None;
        }
        self.accounts.len() != before
    }

    pub fn set_active(&mut self, id: &str) {
        self.active_id = Some(id.to_string());
    }

    pub fn active(&self) -> Option<&Account> {
        self.active_id.as_deref().and_then(|id| self.get(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_temp_dir<F: FnOnce()>(f: F) {
        let dir = std::env::temp_dir().join(format!("cc-switcher-store-{}", uuid::Uuid::new_v4()));
        std::env::set_var("CC_SWITCHER_DIR", &dir);
        f();
        std::env::remove_var("CC_SWITCHER_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn crud_and_persist() {
        with_temp_dir(|| {
            let mut store = Store::load().unwrap();
            assert!(store.accounts.is_empty());

            let id = store.add(Account::new("Аня".into()));
            store.set_active(&id);
            store.save().unwrap();

            // перечитали с диска
            let mut reloaded = Store::load().unwrap();
            assert_eq!(reloaded.accounts.len(), 1);
            assert_eq!(reloaded.active().unwrap().display_name, "Аня");

            // update
            let mut acc = reloaded.get(&id).unwrap().clone();
            acc.display_name = "Анна".into();
            acc.usage = Some(UsageSnapshot {
                percent_remaining: Some(43.0),
                reset_at: Some(now_unix() + 3600),
                reset_label: Some("20:30".into()),
                captured_at: now_unix(),
            });
            assert!(reloaded.update(acc));

            // delete
            assert!(reloaded.delete(&id));
            assert!(reloaded.active_id.is_none());
            assert!(reloaded.accounts.is_empty());
        });
    }
}
