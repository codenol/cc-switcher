//! Token-swap: переключение активного аккаунта Claude Desktop (#4).
//!
//! Последовательность: завершить Claude → резервная копия cookie → подменить
//! cookie сессии выбранного аккаунта → запустить Claude. При ошибке записи —
//! откат из резервной копии.
//!
//! ВНИМАНИЕ: свап завершает процесс Claude Desktop. Если cc-switcher (или эта
//! сессия) работает внутри самого Claude Desktop, запускать свап оттуда нельзя —
//! он завершит и его. Свап рассчитан на отдельное приложение в меню-баре.

use crate::cookies::{self, SessionCookie};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

const CLAUDE_BUNDLE_ID: &str = "com.anthropic.claudefordesktop";
/// Имя процесса главного бинаря Claude Desktop (точное совпадение для pgrep -x).
const CLAUDE_PROCESS: &str = "Claude";

/// Сайдкары WAL-режима SQLite, которые тоже нужно бэкапить/откатывать.
const SIDECARS: [&str; 2] = ["-wal", "-shm"];

/// Этапы свапа — для прогресса в UI (#8/#9).
#[derive(Debug, Clone, Copy)]
pub enum Stage {
    Quitting,
    BackingUp,
    Writing,
    Launching,
    Done,
}

impl Stage {
    pub fn message(self) -> &'static str {
        match self {
            Stage::Quitting => "Завершаю Claude…",
            Stage::BackingUp => "Резервная копия…",
            Stage::Writing => "Подменяю сессию…",
            Stage::Launching => "Запускаю Claude…",
            Stage::Done => "Готово",
        }
    }
}

/// Запущен ли Claude Desktop.
pub fn is_claude_running() -> bool {
    Command::new("pgrep")
        .args(["-x", CLAUDE_PROCESS])
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false)
}

/// Корректно завершить Claude; при таймауте — принудительно.
pub fn quit_claude(timeout: Duration) -> Result<()> {
    if !is_claude_running() {
        return Ok(());
    }
    let _ = Command::new("osascript")
        .args(["-e", r#"tell application "Claude" to quit"#])
        .status();

    let start = Instant::now();
    while is_claude_running() {
        if start.elapsed() > timeout {
            // Принудительно
            let _ = Command::new("pkill").args(["-x", CLAUDE_PROCESS]).status();
            sleep(Duration::from_millis(600));
            if is_claude_running() {
                bail!("не удалось завершить Claude Desktop");
            }
            break;
        }
        sleep(Duration::from_millis(200));
    }
    // Дать ОС дописать WAL и снять локи
    sleep(Duration::from_millis(300));
    Ok(())
}

/// Запустить Claude Desktop.
pub fn launch_claude() -> Result<()> {
    Command::new("open")
        .args(["-b", CLAUDE_BUNDLE_ID])
        .status()
        .context("не удалось запустить Claude Desktop")?;
    Ok(())
}

/// Пути файлов БД cookie вместе с WAL-сайдкарами (только существующие).
fn db_files(db: &Path) -> Vec<PathBuf> {
    let mut files = vec![db.to_path_buf()];
    for s in SIDECARS {
        let mut p = db.as_os_str().to_os_string();
        p.push(s);
        let p = PathBuf::from(p);
        if p.exists() {
            files.push(p);
        }
    }
    files
}

/// Резервная копия БД cookie (+ WAL/SHM). Возвращает пары (оригинал, копия).
pub fn backup_cookies(db: &Path) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut pairs = Vec::new();
    for f in db_files(db) {
        let mut bak = f.as_os_str().to_os_string();
        bak.push(".cc-switcher.bak");
        let bak = PathBuf::from(bak);
        std::fs::copy(&f, &bak)
            .with_context(|| format!("не удалось скопировать {}", f.display()))?;
        pairs.push((f, bak));
    }
    Ok(pairs)
}

/// Восстановить БД cookie из резервной копии (откат).
fn restore_cookies(pairs: &[(PathBuf, PathBuf)]) -> Result<()> {
    for (orig, bak) in pairs {
        std::fs::copy(bak, orig)
            .with_context(|| format!("откат: не удалось восстановить {}", orig.display()))?;
    }
    Ok(())
}

/// Удалить файлы резервной копии (после успешного свапа).
fn cleanup_backups(pairs: &[(PathBuf, PathBuf)]) {
    for (_, bak) in pairs {
        let _ = std::fs::remove_file(bak);
    }
}

/// Полный цикл переключения на заданный набор cookie.
///
/// `progress` вызывается на каждом этапе (для событий в UI). Claude должен быть
/// закрыт по ходу выполнения — функция делает это сама.
pub fn switch_to<F: Fn(Stage)>(
    key: &[u8; 16],
    cookies: &[SessionCookie],
    progress: F,
) -> Result<()> {
    if cookies.is_empty() {
        bail!("нет cookie для переключения");
    }
    let db = cookies::claude_cookies_path();
    if !db.exists() {
        bail!("не найдена база cookie Claude: {}", db.display());
    }

    progress(Stage::Quitting);
    quit_claude(Duration::from_secs(10))?;

    progress(Stage::BackingUp);
    let backups = backup_cookies(&db)?;

    progress(Stage::Writing);
    if let Err(e) = cookies::write_session_cookies(&db, key, cookies) {
        // Откат и выход
        let _ = restore_cookies(&backups);
        cleanup_backups(&backups);
        return Err(e.context("запись cookie не удалась — выполнен откат"));
    }
    cleanup_backups(&backups);

    progress(Stage::Launching);
    launch_claude()?;

    progress(Stage::Done);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_process_does_not_panic() {
        // Просто не должно паниковать и должно вернуть bool.
        let _ = is_claude_running();
    }

    #[test]
    fn backup_and_restore_roundtrip() {
        // Безопасный тест на временном файле, не трогает реальные cookie.
        let dir = std::env::temp_dir().join("cc-switcher-swap-test");
        let _ = std::fs::create_dir_all(&dir);
        let db = dir.join("Cookies");
        std::fs::write(&db, b"ORIGINAL").unwrap();
        // имитируем WAL
        std::fs::write(dir.join("Cookies-wal"), b"WAL").unwrap();

        let pairs = backup_cookies(&db).unwrap();
        assert!(pairs.len() >= 1);

        // портим оригинал и откатываем
        std::fs::write(&db, b"CORRUPT").unwrap();
        restore_cookies(&pairs).unwrap();
        assert_eq!(std::fs::read(&db).unwrap(), b"ORIGINAL");

        cleanup_backups(&pairs);
        for (_, bak) in &pairs {
            assert!(!bak.exists(), "копия должна быть удалена");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
