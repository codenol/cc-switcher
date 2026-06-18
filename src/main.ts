// Окно настроек: визуальный CRUD аккаунтов (#10).
import { invoke } from "@tauri-apps/api/core";

interface UsageSnapshot {
  percent_remaining: number | null;
  reset_at: number | null;
  captured_at: number;
}

interface AccountView {
  id: string;
  display_name: string;
  email: string;
  email_url: string;
  has_cookies: boolean;
  session_expires_utc: number | null;
  usage: UsageSnapshot | null;
  is_active: boolean;
}

interface AccountSecrets {
  password: string | null;
  email_password: string | null;
}

let accounts: AccountView[] = [];
let selectedId: string | null = null;

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const listEl = $("account-list");
const formEl = $<HTMLFormElement>("account-form");
const emptyEl = $("empty-state");
const cookieStatusEl = $("cookie-status");
const hintEl = $("form-hint");

const fName = $<HTMLInputElement>("f-name");
const fEmail = $<HTMLInputElement>("f-email");
const fPassword = $<HTMLInputElement>("f-password");
const fEmailPassword = $<HTMLInputElement>("f-email-password");
const fEmailUrl = $<HTMLInputElement>("f-email-url");

function toast(msg: string) {
  const el = $("toast");
  el.textContent = msg;
  el.hidden = false;
  window.clearTimeout((el as any)._t);
  (el as any)._t = window.setTimeout(() => (el.hidden = true), 2600);
}

// Chromium WebKit-время cookie (микросекунды от 1601) → человекочитаемая дата.
function chromeTimeToDate(t: number | null): string {
  if (!t) return "—";
  const ms = t / 1000 - 11644473600000;
  return new Date(ms).toLocaleString("ru-RU", { dateStyle: "medium" });
}

async function refresh() {
  accounts = await invoke<AccountView[]>("list_accounts");
  renderList();
  if (selectedId && !accounts.some((a) => a.id === selectedId)) {
    selectedId = null;
  }
  renderForm();
}

function renderList() {
  listEl.innerHTML = "";
  for (const a of accounts) {
    const li = document.createElement("li");
    li.className = "account-item" + (a.id === selectedId ? " selected" : "");
    li.onclick = () => selectAccount(a.id);
    const cookieDot = `<span class="dot ${a.has_cookies ? "ok" : ""}" title="${
      a.has_cookies ? "cookie захвачены" : "cookie не захвачены"
    }"></span>`;
    const badge = a.is_active ? `<span class="badge">активен</span>` : "";
    li.innerHTML = `
      <div class="name">${cookieDot}${escapeHtml(a.display_name)}${badge}</div>
      <div class="email">${escapeHtml(a.email)}</div>`;
    listEl.appendChild(li);
  }
}

function escapeHtml(s: string): string {
  const d = document.createElement("div");
  d.textContent = s;
  return d.innerHTML;
}

async function selectAccount(id: string) {
  selectedId = id;
  renderList();
  renderForm();
  // подгрузить секреты в форму
  const secrets = await invoke<AccountSecrets>("get_account_secrets", { id });
  fPassword.value = secrets.password ?? "";
  fEmailPassword.value = secrets.email_password ?? "";
}

function renderForm() {
  const acc = accounts.find((a) => a.id === selectedId) ?? null;
  const isNew = selectedId === "__new__";

  if (!acc && !isNew) {
    formEl.hidden = true;
    emptyEl.hidden = false;
    return;
  }
  formEl.hidden = false;
  emptyEl.hidden = true;
  hintEl.textContent = "";

  fName.value = acc?.display_name ?? "";
  fEmail.value = acc?.email ?? "";
  fEmailUrl.value = acc?.email_url ?? "";
  if (isNew) {
    fPassword.value = "";
    fEmailPassword.value = "";
  }

  if (isNew) {
    cookieStatusEl.textContent = "Новый аккаунт — сохраните, затем захватите cookie.";
  } else if (acc?.has_cookies) {
    cookieStatusEl.textContent = `Cookie захвачены · sessionKey действует до ${chromeTimeToDate(
      acc.session_expires_utc
    )}`;
  } else {
    cookieStatusEl.textContent = "Cookie не захвачены.";
  }
}

function newAccount() {
  selectedId = "__new__";
  renderList();
  renderForm();
  fName.focus();
}

async function saveAccount(e: Event) {
  e.preventDefault();
  const input = {
    id: selectedId === "__new__" ? null : selectedId,
    display_name: fName.value.trim(),
    email: fEmail.value.trim(),
    email_url: fEmailUrl.value.trim(),
    password: fPassword.value,
    email_password: fEmailPassword.value,
  };
  try {
    const id = await invoke<string>("save_account", { input });
    selectedId = id;
    await refresh();
    toast("Сохранено");
  } catch (err) {
    hintEl.textContent = String(err);
  }
}

async function deleteAccount() {
  if (!selectedId || selectedId === "__new__") return;
  const acc = accounts.find((a) => a.id === selectedId);
  if (!confirm(`Удалить аккаунт «${acc?.display_name}»? Секреты тоже будут удалены.`)) return;
  await invoke("delete_account", { id: selectedId });
  selectedId = null;
  await refresh();
  toast("Удалён");
}

async function captureAccount() {
  if (!selectedId || selectedId === "__new__") {
    hintEl.textContent = "Сначала сохраните аккаунт, потом захватывайте cookie.";
    return;
  }
  try {
    await invoke("capture_account", { id: selectedId });
    await refresh();
    await selectAccount(selectedId);
    toast("Cookie захвачены");
  } catch (err) {
    hintEl.textContent = String(err);
  }
}

window.addEventListener("DOMContentLoaded", () => {
  $("btn-new").onclick = newAccount;
  $("btn-delete").onclick = deleteAccount;
  $("btn-capture").onclick = captureAccount;
  formEl.addEventListener("submit", saveAccount);
  refresh();
});
