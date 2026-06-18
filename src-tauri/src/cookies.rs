//! Cookie-движок: чтение, де- и перешифровка cookie сессии Claude Desktop (#3).
//!
//! Claude Desktop — Electron/Chromium. Авторизация claude.ai лежит в SQLite
//! `~/Library/Application Support/Claude/Cookies`. Значения cookie зашифрованы
//! схемой Chromium safeStorage:
//!   - ключ-пароль — в Keychain (service «Claude Safe Storage», account «Claude Key»);
//!   - производный ключ = PBKDF2-HMAC-SHA1(пароль, salt="saltysalt", iter=1003, 16 байт);
//!   - значение = "v10" ++ AES-128-CBC(IV = 16×0x20, PKCS7);
//!   - расшифрованный plaintext начинается с 32 байт SHA256(host_key), затем само значение.
//!
//! Всё подтверждено эмпирически на реальных данных этой машины.

use aes::Aes128;
use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

type AesCbcEnc = cbc::Encryptor<Aes128>;
type AesCbcDec = cbc::Decryptor<Aes128>;

const SALT: &[u8] = b"saltysalt";
const ITERATIONS: u32 = 1003;
const IV: [u8; 16] = [0x20; 16];

/// Имя cookie, несущих авторизацию. `sessionKey` — главный; остальные сопутствуют.
pub const SESSION_COOKIE_NAMES: [&str; 3] = ["sessionKey", "sessionKeyLC", "lastActiveOrg"];
/// Домен, под которым живут cookie Claude.
pub const HOST: &str = ".claude.ai";

/// Путь к базе cookie Claude Desktop.
pub fn claude_cookies_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join("Library/Application Support/Claude/Cookies")
}

// ───────────────────────── Keychain + ключ ─────────────────────────

/// Прочитать пароль safeStorage из системной связки ключей.
/// При первом обращении macOS показывает разовый диалог → «Always Allow».
#[cfg(target_os = "macos")]
pub fn safe_storage_password() -> Result<Vec<u8>> {
    security_framework::passwords::get_generic_password("Claude Safe Storage", "Claude Key")
        .context("не удалось прочитать ключ «Claude Safe Storage» из Keychain")
}

#[cfg(not(target_os = "macos"))]
pub fn safe_storage_password() -> Result<Vec<u8>> {
    bail!("cookie-движок поддерживается только на macOS")
}

/// Вывести 16-байтовый AES-ключ из пароля safeStorage.
pub fn derive_key(password: &[u8]) -> [u8; 16] {
    let mut key = [0u8; 16];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password, SALT, ITERATIONS, &mut key);
    key
}

/// Удобный шорткат: достать пароль из Keychain и вывести ключ.
pub fn load_key() -> Result<[u8; 16]> {
    Ok(derive_key(&safe_storage_password()?))
}

// ───────────────────────── Шифрование ─────────────────────────

/// Расшифровать значение cookie (`v10…`) в полный plaintext
/// (включая 32-байтовый префикс SHA256(host)).
pub fn decrypt(encrypted: &[u8], key: &[u8; 16]) -> Result<Vec<u8>> {
    if encrypted.len() < 3 || &encrypted[..3] != b"v10" {
        bail!("неизвестный формат cookie (ожидался префикс v10)");
    }
    let ct = &encrypted[3..];
    if ct.is_empty() || ct.len() % 16 != 0 {
        bail!("длина шифртекста не кратна 16");
    }
    let mut buf = ct.to_vec();
    let pt = AesCbcDec::new(key.into(), &IV.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|e| anyhow!("ошибка расшифровки/снятия PKCS7: {e}"))?;
    Ok(pt.to_vec())
}

/// Зашифровать полный plaintext обратно в формат `v10…`.
pub fn encrypt(plaintext: &[u8], key: &[u8; 16]) -> Vec<u8> {
    let ct = AesCbcEnc::new(key.into(), &IV.into()).encrypt_padded_vec_mut::<Pkcs7>(plaintext);
    let mut out = Vec::with_capacity(3 + ct.len());
    out.extend_from_slice(b"v10");
    out.extend_from_slice(&ct);
    out
}

/// 32-байтовый префикс, который Chromium добавляет к значению: SHA256(host_key).
pub fn host_prefix(host: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(host.as_bytes());
    h.finalize().into()
}

/// Извлечь чистое значение cookie из полного plaintext (отрезать префикс host).
pub fn value_from_plaintext(plaintext: &[u8]) -> Result<String> {
    if plaintext.len() < 32 {
        bail!("plaintext короче 32-байтового префикса");
    }
    String::from_utf8(plaintext[32..].to_vec()).context("значение cookie не UTF-8")
}

// ───────────────────────── Чтение / запись БД ─────────────────────────

/// Одна строка cookie со всеми колонками БД. `encrypted_b64` — это родной
/// Chromium-блоб `v10…` (base64) — источник истины, зашифрован ключом машины.
/// Ключ safeStorage один на всю машину, поэтому блоб можно переносить между
/// аккаунтами как есть, без перешифровки. Расшифровка нужна только чтобы
/// получить значение (`value`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCookie {
    pub name: String,
    pub encrypted_b64: String,
    pub creation_utc: i64,
    pub host_key: String,
    pub top_frame_site_key: String,
    pub path: String,
    pub expires_utc: i64,
    pub is_secure: i64,
    pub is_httponly: i64,
    pub last_access_utc: i64,
    pub has_expires: i64,
    pub is_persistent: i64,
    pub priority: i64,
    pub samesite: i64,
    pub source_scheme: i64,
    pub source_port: i64,
    pub last_update_utc: i64,
    pub source_type: i64,
    pub has_cross_site_ancestor: i64,
}

impl SessionCookie {
    /// Чистое значение cookie (например, сам `sk-ant-…` для sessionKey).
    /// Требует ключ машины — расшифровывает блоб и отрезает префикс host.
    pub fn value(&self, key: &[u8; 16]) -> Result<String> {
        let enc = B64.decode(&self.encrypted_b64).context("encrypted_b64 не base64")?;
        let pt = decrypt(&enc, key)?;
        value_from_plaintext(&pt)
    }
}

/// Прочитать cookie сессии из базы Claude в виде зашифрованных блобов.
/// Ключ не нужен: блоб хранится и переносится как есть.
/// Возвращает только реально присутствующие из [`SESSION_COOKIE_NAMES`].
pub fn read_session_cookies(db_path: &PathBuf) -> Result<Vec<SessionCookie>> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("не удалось открыть {}", db_path.display()))?;

    let mut out = Vec::new();
    for name in SESSION_COOKIE_NAMES {
        let row = conn
            .query_row(
                "SELECT creation_utc, host_key, top_frame_site_key, name, encrypted_value, path, \
                 expires_utc, is_secure, is_httponly, last_access_utc, has_expires, is_persistent, \
                 priority, samesite, source_scheme, source_port, last_update_utc, source_type, \
                 has_cross_site_ancestor \
                 FROM cookies WHERE name = ?1 AND host_key = ?2",
                params![name, HOST],
                row_to_cookie,
            )
            .ok();

        if let Some(c) = row {
            out.push(c);
        }
    }
    Ok(out)
}

/// Собрать [`SessionCookie`] из строки выборки (блоб encrypted_value → base64).
fn row_to_cookie(r: &rusqlite::Row) -> rusqlite::Result<SessionCookie> {
    let enc: Vec<u8> = r.get(4)?;
    Ok(SessionCookie {
        name: r.get(3)?,
        encrypted_b64: B64.encode(enc),
        creation_utc: r.get(0)?,
        host_key: r.get(1)?,
        top_frame_site_key: r.get(2)?,
        path: r.get(5)?,
        expires_utc: r.get(6)?,
        is_secure: r.get(7)?,
        is_httponly: r.get(8)?,
        last_access_utc: r.get(9)?,
        has_expires: r.get(10)?,
        is_persistent: r.get(11)?,
        priority: r.get(12)?,
        samesite: r.get(13)?,
        source_scheme: r.get(14)?,
        source_port: r.get(15)?,
        last_update_utc: r.get(16)?,
        source_type: r.get(17)?,
        has_cross_site_ancestor: r.get(18)?,
    })
}

/// Записать (UPSERT) cookie сессии в базу Claude. Блоб `encrypted_b64`
/// пишется как есть — перешифровка не нужна (ключ машины общий).
/// Claude должен быть закрыт. `value` хранится пустым — данные в `encrypted_value`.
pub fn write_session_cookies(db_path: &PathBuf, cookies: &[SessionCookie]) -> Result<()> {
    let mut conn = Connection::open(db_path)
        .with_context(|| format!("не удалось открыть на запись {}", db_path.display()))?;
    let tx = conn.transaction()?;
    for c in cookies {
        let enc = B64.decode(&c.encrypted_b64).context("encrypted_b64 не base64")?;
        tx.execute(
            "INSERT OR REPLACE INTO cookies \
             (creation_utc, host_key, top_frame_site_key, name, value, encrypted_value, path, \
              expires_utc, is_secure, is_httponly, last_access_utc, has_expires, is_persistent, \
              priority, samesite, source_scheme, source_port, last_update_utc, source_type, \
              has_cross_site_ancestor) \
             VALUES (?1, ?2, ?3, ?4, '', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
            params![
                c.creation_utc, c.host_key, c.top_frame_site_key, c.name, enc, c.path,
                c.expires_utc, c.is_secure, c.is_httponly, c.last_access_utc, c.has_expires,
                c.is_persistent, c.priority, c.samesite, c.source_scheme, c.source_port,
                c.last_update_utc, c.source_type, c.has_cross_site_ancestor,
            ],
        )
        .with_context(|| format!("не удалось записать cookie {}", c.name))?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_roundtrip_synthetic() {
        let key = derive_key(b"some-test-password");
        let mut plaintext = host_prefix(HOST).to_vec();
        plaintext.extend_from_slice(b"sk-ant-sid02-EXAMPLE-VALUE");
        let enc = encrypt(&plaintext, &key);
        assert_eq!(&enc[..3], b"v10");
        let dec = decrypt(&enc, &key).unwrap();
        assert_eq!(dec, plaintext);
        assert_eq!(value_from_plaintext(&dec).unwrap(), "sk-ant-sid02-EXAMPLE-VALUE");
    }

    /// Боевой тест на реальной базе и Keychain этой машины.
    /// Требует доступа к Keychain (разовый промпт). Запуск:
    ///   cargo test --manifest-path src-tauri/Cargo.toml real_ -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_read_and_roundtrip() {
        let key = load_key().expect("ключ из Keychain");
        let path = claude_cookies_path();
        let cookies = read_session_cookies(&path).expect("чтение cookie");
        assert!(!cookies.is_empty(), "не найдено ни одной session-cookie");

        let sk = cookies.iter().find(|c| c.name == "sessionKey").expect("есть sessionKey");
        let val = sk.value(&key).expect("значение sessionKey");
        println!("sessionKey начинается с: {}", &val[..val.len().min(12)]);
        assert!(val.starts_with("sk-ant"), "значение не похоже на sessionKey: {val:.12}");

        // round-trip: блоб расшифровывается в осмысленное значение
        let enc = B64.decode(&sk.encrypted_b64).unwrap();
        let re = decrypt(&enc, &key).unwrap();
        assert_eq!(&re[..32], &host_prefix(HOST), "префикс host не совпал");
        println!("OK: прочитано {} cookie, значение валидно", cookies.len());
    }
}
