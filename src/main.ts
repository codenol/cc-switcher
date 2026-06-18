// Окно настроек: визуальный CRUD аккаунтов (#10).
// Это же окно в режиме `#prompt` — диалог ввода времени ресета (#7 v2).
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

interface UsageSnapshot {
  percent_remaining: number | null;
  reset_at: number | null;
  reset_label: string | null;
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

function initSettings() {
  $("btn-new").onclick = newAccount;
  $("btn-delete").onclick = deleteAccount;
  $("btn-capture").onclick = captureAccount;
  formEl.addEventListener("submit", saveAccount);
  refresh();
}

// ───────────────────── Режим промпта (#7 v2) ─────────────────────

interface SwitchPrompt {
  target_id: string;
  target_name: string;
  outgoing_id: string | null;
  outgoing_name: string | null;
  already_active: boolean;
}

const pad = (n: number) => String(n).padStart(2, "0");

// «HH:MM» (локальное) → ближайшая будущая unix-метка + нормализованная подпись.
function computeReset(timeStr: string): { reset_at: number; label: string } | null {
  if (!timeStr) return null;
  const [h, m] = timeStr.split(":").map(Number);
  if (Number.isNaN(h) || Number.isNaN(m)) return null;
  const d = new Date();
  d.setHours(h, m, 0, 0);
  if (d.getTime() <= Date.now()) d.setDate(d.getDate() + 1);
  return { reset_at: Math.floor(d.getTime() / 1000), label: `${pad(h)}:${pad(m)}` };
}

async function initPrompt() {
  $("prompt-view").hidden = false;
  const appEl = document.querySelector<HTMLElement>(".app");
  if (appEl) appEl.hidden = true;

  const titleEl = $("p-title");
  const msgEl = $("p-msg");
  const statusEl = $("p-status");
  const timeRow = $("p-time-row");
  const timeEl = $<HTMLInputElement>("p-time");
  const primary = $<HTMLButtonElement>("p-primary");
  const secondary = $<HTMLButtonElement>("p-secondary");
  const win = getCurrentWindow();
  const close = () => win.close();

  let prompt: SwitchPrompt | null = await invoke<SwitchPrompt | null>("get_pending_prompt");
  // существующие подписи ресета (для предзаполнения)
  const labelById = new Map<string, string>();
  for (const a of await invoke<AccountView[]>("list_accounts")) {
    if (a.usage?.reset_label) labelById.set(a.id, a.usage.reset_label);
  }

  if (!prompt) {
    close();
    return;
  }

  // Шаг «после переключения»: задать время ресета целевого аккаунта.
  function showSetReset(id: string, name: string, lead: string) {
    titleEl.textContent = "Время ресета";
    msgEl.textContent = `${lead} Когда сбрасывается лимит сессии «${name}»?`;
    statusEl.textContent = "";
    timeRow.hidden = false;
    timeEl.value = labelById.get(id) ?? "";
    primary.textContent = "Сохранить";
    secondary.textContent = "Пропустить";
    primary.disabled = false;
    secondary.disabled = false;
    primary.onclick = async () => {
      const r = computeReset(timeEl.value);
      if (r) await invoke("set_reset_time", { id, resetAt: r.reset_at, label: r.label });
      close();
    };
    secondary.onclick = close;
    timeEl.focus();
  }

  // Выполнить свап на целевой аккаунт, затем предложить его время ресета.
  function runSwitch(p: SwitchPrompt) {
    titleEl.textContent = "Переключение";
    msgEl.textContent = `Переключаюсь на «${p.target_name}»…`;
    timeRow.hidden = true;
    statusEl.textContent = "Завершаю Claude…";
    primary.disabled = true;
    secondary.disabled = true;
    invoke("switch_account", { id: p.target_id });
  }

  await listen<string>("switch-progress", (e) => {
    statusEl.textContent = e.payload;
  });
  await listen<string>("switch-done", () => {
    showSetReset(prompt!.target_id, prompt!.target_name, "Готово, сессия запущена.");
  });
  await listen<string>("switch-error", (e) => {
    titleEl.textContent = "Не получилось";
    msgEl.textContent = "Переключение не удалось.";
    timeRow.hidden = true;
    statusEl.textContent = String(e.payload);
    primary.textContent = "Закрыть";
    primary.disabled = false;
    primary.onclick = close;
    secondary.hidden = true;
  });

  // Перечитать запрос, если окно переиспользовали для нового переключения.
  await listen("prompt-show", async () => {
    const fresh = await invoke<SwitchPrompt | null>("get_pending_prompt");
    if (fresh) {
      prompt = fresh;
      secondary.hidden = false;
      route(fresh);
    }
  });

  function route(p: SwitchPrompt) {
    if (p.already_active) {
      // целевой уже активен — свап не нужен, только задать его ресет
      showSetReset(p.target_id, p.target_name, "Аккаунт уже активен.");
    } else if (p.outgoing_id && p.outgoing_name) {
      // шаг «до закрытия»: ресет уходящего, затем свап
      const outId = p.outgoing_id;
      titleEl.textContent = "Перед переключением";
      msgEl.textContent = `Когда сбрасывается лимит у «${p.outgoing_name}»?`;
      statusEl.textContent = "";
      timeRow.hidden = false;
      timeEl.value = labelById.get(outId) ?? "";
      primary.textContent = "Сохранить и переключить";
      secondary.textContent = "Переключить без времени";
      primary.disabled = false;
      secondary.disabled = false;
      primary.onclick = async () => {
        const r = computeReset(timeEl.value);
        if (r) await invoke("set_reset_time", { id: outId, resetAt: r.reset_at, label: r.label });
        runSwitch(p);
      };
      secondary.onclick = () => runSwitch(p);
      timeEl.focus();
    } else {
      // активного нет — просто переключаемся, потом спросим ресет нового
      runSwitch(p);
    }
  }

  route(prompt);
}

window.addEventListener("DOMContentLoaded", () => {
  if (location.hash === "#prompt") {
    initPrompt();
  } else {
    initSettings();
  }
});
