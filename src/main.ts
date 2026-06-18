// Окно настроек: визуальный CRUD аккаунтов (#10).
import { invoke } from "@tauri-apps/api/core";

interface UsageSnapshot {
  percent_remaining: number | null;
  reset_at: number | null;
  reset_label: string | null;
  captured_at: number;
}

interface AccountView {
  id: string;
  display_name: string;
  has_cookies: boolean;
  session_expires_utc: number | null;
  usage: UsageSnapshot | null;
  is_active: boolean;
}

let accounts: AccountView[] = [];
let selectedId: string | null = null;

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const tabbarEl = $("tabbar");
const tabsEl = $("tabs");
const formEl = $<HTMLFormElement>("account-form");
const emptyEl = $("empty-state");
const cookieStatusEl = $("cookie-status");
const activeBadgeEl = $("active-badge");
const hintEl = $("form-hint");

const fName = $<HTMLInputElement>("f-name");
const fReset = $<HTMLInputElement>("f-reset");

// «HH:MM» (локальное) → ближайшая будущая unix-метка + нормализованная подпись.
function computeReset(timeStr: string): { reset_at: number; label: string } | null {
  if (!timeStr) return null;
  const [h, m] = timeStr.split(":").map(Number);
  if (Number.isNaN(h) || Number.isNaN(m)) return null;
  const d = new Date();
  d.setHours(h, m, 0, 0);
  if (d.getTime() <= Date.now()) d.setDate(d.getDate() + 1);
  const pad = (n: number) => String(n).padStart(2, "0");
  return { reset_at: Math.floor(d.getTime() / 1000), label: `${pad(h)}:${pad(m)}` };
}

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
  // выбранный аккаунт мог исчезнуть
  if (selectedId && selectedId !== "__new__" && !accounts.some((a) => a.id === selectedId)) {
    selectedId = null;
  }
  // по умолчанию открыть активный (или первый) аккаунт
  if (!selectedId && accounts.length > 0) {
    const def = accounts.find((a) => a.is_active) ?? accounts[0];
    selectAccount(def.id);
    return;
  }
  renderTabs();
  renderForm();
}

function renderTabs() {
  const hasAccounts = accounts.length > 0;
  tabbarEl.hidden = !hasAccounts && selectedId !== "__new__";
  tabsEl.innerHTML = "";
  for (const a of accounts) {
    const b = document.createElement("button");
    b.className = "tab" + (a.id === selectedId ? " selected" : "");
    b.onclick = () => selectAccount(a.id);
    const cookieDot = `<span class="dot ${a.has_cookies ? "ok" : ""}" title="${
      a.has_cookies ? "cookie захвачены" : "cookie не захвачены"
    }"></span>`;
    const live = a.is_active ? `<span class="live" title="активен в Claude"></span>` : "";
    b.innerHTML = `${cookieDot}${escapeHtml(a.display_name)}${live}`;
    tabsEl.appendChild(b);
  }
}

function escapeHtml(s: string): string {
  const d = document.createElement("div");
  d.textContent = s;
  return d.innerHTML;
}

function selectAccount(id: string) {
  selectedId = id;
  renderTabs();
  renderForm();
}

function renderForm() {
  const acc = accounts.find((a) => a.id === selectedId) ?? null;
  const isNew = selectedId === "__new__";

  if (!acc && !isNew) {
    formEl.hidden = true;
    // центральная кнопка — только когда аккаунтов нет вовсе
    emptyEl.hidden = accounts.length > 0;
    return;
  }
  formEl.hidden = false;
  emptyEl.hidden = true;
  hintEl.textContent = "";

  fName.value = acc?.display_name ?? "";
  fReset.value = acc?.usage?.reset_label ?? ""; // необязательное

  // бейдж активности в Claude
  activeBadgeEl.hidden = !acc?.is_active;

  // статус cookie с цветовым акцентом
  cookieStatusEl.classList.remove("ok", "warn");
  if (isNew) {
    cookieStatusEl.textContent = "Новый аккаунт — сохраните, затем захватите cookie.";
    cookieStatusEl.classList.add("warn");
  } else if (acc?.has_cookies) {
    cookieStatusEl.textContent = `● Cookie захвачены · действуют до ${chromeTimeToDate(
      acc.session_expires_utc
    )}`;
    cookieStatusEl.classList.add("ok");
  } else {
    cookieStatusEl.textContent = "● Cookie не захвачены";
    cookieStatusEl.classList.add("warn");
  }
}

function newAccount() {
  selectedId = "__new__";
  renderTabs();
  renderForm();
  fName.focus();
}

async function saveAccount(e: Event) {
  e.preventDefault();
  const input = {
    id: selectedId === "__new__" ? null : selectedId,
    display_name: fName.value.trim(),
  };
  try {
    const id = await invoke<string>("save_account", { input });
    // время ресета — необязательное: задать, если ввели, иначе очистить
    const r = computeReset(fReset.value);
    if (r) {
      await invoke("set_reset_time", { id, resetAt: r.reset_at, label: r.label });
    } else {
      await invoke("clear_reset_time", { id });
    }
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
  if (!confirm(`Удалить аккаунт «${acc?.display_name}»?`)) return;
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
  $("btn-first").onclick = newAccount;
  $("btn-delete").onclick = deleteAccount;
  $("btn-capture").onclick = captureAccount;
  formEl.addEventListener("submit", saveAccount);
  refresh();
});
