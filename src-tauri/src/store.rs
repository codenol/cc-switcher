//! Хранилище аккаунтов (#5): модель, CRUD, секреты в Keychain, снимки usage.
//!
//! Несекретные данные (аккаунты, cookie-блобы, снимок usage) — в JSON
//! `~/Library/Application Support/cc-switcher/accounts.json`.
//! Пароли и пароли почты — только в Keychain (service «cc-switcher»),
//! в JSON их нет. Cookie лежат как зашифрованные блобы (см. [`crate::cookies`]).

use crate::cookies::SessionCookie;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const KEYCHAIN_SERVICE: &str = "cc-switcher";

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

/// Аккаунт. Секреты (пароли) здесь не хранятся — они в Keychain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub display_name: String,
    /// Логин — почта.
    pub email: String,
    /// Ссылка на почтовый сервис (для ручного релогина).
    #[serde(default)]
    pub email_url: String,
    /// Зашифрованные cookie сессии (блобы v10).
    #[serde(default)]
    pub cookies: Vec<SessionCookie>,
    /// Последний известный снимок usage.
    #[serde(default)]
    pub usage: Option<UsageSnapshot>,
}

impl Account {
    pub fn new(display_name: String, email: String, email_url: String) -> Self {
        Account {
            id: uuid::Uuid::new_v4().to_string(),
            display_name,
            email,
            email_url,
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

    /// Удалить аккаунт и его секреты из Keychain.
    pub fn delete(&mut self, id: &str) -> bool {
        let before = self.accounts.len();
        self.accounts.retain(|a| a.id != id);
        if self.active_id.as_deref() == Some(id) {
            self.active_id = None;
        }
        let removed = self.accounts.len() != before;
        if removed {
            let _ = delete_secret(id, SecretKind::Password);
            let _ = delete_secret(id, SecretKind::EmailPassword);
        }
        removed
    }

    pub fn set_active(&mut self, id: &str) {
        self.active_id = Some(id.to_string());
    }

    pub fn active(&self) -> Option<&Account> {
        self.active_id.as_deref().and_then(|id| self.get(id))
    }
}

// ───────────────────────── Секреты в Keychain ─────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum SecretKind {
    Password,
    EmailPassword,
}

impl SecretKind {
    fn suffix(self) -> &'static str {
        match self {
            SecretKind::Password => "password",
            SecretKind::EmailPassword => "email_password",
        }
    }
}

fn kc_account(account_id: &str, kind: SecretKind) -> String {
    format!("{account_id}:{}", kind.suffix())
}

/// Сохранить секрет аккаунта в Keychain.
#[cfg(target_os = "macos")]
pub fn set_secret(account_id: &str, kind: SecretKind, secret: &str) -> Result<()> {
    security_framework::passwords::set_generic_password(
        KEYCHAIN_SERVICE,
        &kc_account(account_id, kind),
        secret.as_bytes(),
    )
    .context("не удалось записать секрет в Keychain")
}

/// Прочитать секрет аккаунта из Keychain (None, если не задан).
#[cfg(target_os = "macos")]
pub fn get_secret(account_id: &str, kind: SecretKind) -> Result<Option<String>> {
    match security_framework::passwords::get_generic_password(
        KEYCHAIN_SERVICE,
        &kc_account(account_id, kind),
    ) {
        Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
        Err(_) => Ok(None),
    }
}

/// Удалить секрет аккаунта из Keychain.
#[cfg(target_os = "macos")]
pub fn delete_secret(account_id: &str, kind: SecretKind) -> Result<()> {
    let _ = security_framework::passwords::delete_generic_password(
        KEYCHAIN_SERVICE,
        &kc_account(account_id, kind),
    );
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn set_secret(_: &str, _: SecretKind, _: &str) -> Result<()> {
    anyhow::bail!("Keychain доступен только на macOS")
}
#[cfg(not(target_os = "macos"))]
pub fn get_secret(_: &str, _: SecretKind) -> Result<Option<String>> {
    Ok(None)
}
#[cfg(not(target_os = "macos"))]
pub fn delete_secret(_: &str, _: SecretKind) -> Result<()> {
    Ok(())
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

            let id = store.add(Account::new(
                "Аня".into(),
                "anya@example.com".into(),
                "https://mail.example.com".into(),
            ));
            store.set_active(&id);
            store.save().unwrap();

            // перечитали с диска
            let mut reloaded = Store::load().unwrap();
            assert_eq!(reloaded.accounts.len(), 1);
            assert_eq!(reloaded.active().unwrap().email, "anya@example.com");

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

    /// Боевой тест Keychain (пишет/читает/удаляет временный секрет). Запуск:
    ///   cargo test --manifest-path src-tauri/Cargo.toml store::tests::secret_ -- --ignored
    #[test]
    #[ignore]
    fn secret_roundtrip_keychain() {
        let id = format!("test-{}", uuid::Uuid::new_v4());
        set_secret(&id, SecretKind::Password, "hunter2").unwrap();
        assert_eq!(
            get_secret(&id, SecretKind::Password).unwrap().as_deref(),
            Some("hunter2")
        );
        delete_secret(&id, SecretKind::Password).unwrap();
        assert!(get_secret(&id, SecretKind::Password).unwrap().is_none());
    }
}
