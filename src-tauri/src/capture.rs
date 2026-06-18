//! Захват текущего аккаунта (#6): единственное место, где нужен ручной логин.
//!
//! Человек логинится в Claude Desktop обычным способом, затем мы считываем
//! cookie активной сессии и сохраняем их за аккаунтом. Авто-определение email
//! через API claude.ai недоступно: эндпоинты за Cloudflare-челленджем, который
//! не проходит сторонний HTTP-клиент (см. #7). Поэтому имя и почту вводит
//! человек в окне настроек.

use crate::cookies::{self, SessionCookie};
use anyhow::{bail, Result};

/// Считать cookie активной сессии Claude Desktop.
/// Ошибка, если активной сессии нет (нет `sessionKey`).
pub fn capture_cookies() -> Result<Vec<SessionCookie>> {
    let path = cookies::claude_cookies_path();
    if !path.exists() {
        bail!("не найдена база cookie Claude: {}", path.display());
    }
    let cookies = cookies::read_session_cookies(&path)?;
    if !cookies.iter().any(|c| c.name == "sessionKey") {
        bail!("нет активной сессии Claude — войдите в нужный аккаунт и повторите");
    }
    Ok(cookies)
}

/// Сводка о захваченной сессии для подтверждения в UI.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CaptureInfo {
    /// Сколько cookie захвачено.
    pub cookie_count: usize,
    /// Срок жизни sessionKey (unix-секунды), если удалось прочитать.
    pub session_expires_utc: Option<i64>,
    /// Префикс значения sessionKey для визуальной проверки (например, `sk-ant-sid02`).
    pub session_value_preview: Option<String>,
}

/// Прочитать cookie и собрать сводку. `key` нужен только для превью значения;
/// при ошибке расшифровки превью опускается (сам захват не страдает).
pub fn capture_info(key: Option<&[u8; 16]>) -> Result<(Vec<SessionCookie>, CaptureInfo)> {
    let cookies = capture_cookies()?;
    let sk = cookies.iter().find(|c| c.name == "sessionKey");

    let session_expires_utc = sk.map(|c| c.expires_utc);
    let session_value_preview = match (key, sk) {
        (Some(k), Some(c)) => c.value(k).ok().map(|v| {
            let n = v.len().min(12);
            v[..n].to_string()
        }),
        _ => None,
    };

    let info = CaptureInfo {
        cookie_count: cookies.len(),
        session_expires_utc,
        session_value_preview,
    };
    Ok((cookies, info))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Боевой: захват cookie из реальной запущенной сессии Claude.
    /// Запуск: cargo test --manifest-path src-tauri/Cargo.toml capture:: -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_capture() {
        let key = cookies::load_key().ok();
        let (cookies, info) = capture_info(key.as_ref()).expect("захват сессии");
        assert!(cookies.iter().any(|c| c.name == "sessionKey"));
        println!(
            "Захвачено {} cookie; sessionKey preview = {:?}, expires_utc = {:?}",
            info.cookie_count, info.session_value_preview, info.session_expires_utc
        );
        assert_eq!(info.session_value_preview.as_deref(), Some("sk-ant-sid02"));
    }
}
