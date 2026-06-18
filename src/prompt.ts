// Отдельное окно: диалог ввода времени ресета вокруг переключения (#7 v2, #20).
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

interface SwitchPrompt {
  target_id: string;
  target_name: string;
  outgoing_id: string | null;
  outgoing_name: string | null;
  already_active: boolean;
}

interface UsageSnapshot {
  reset_label: string | null;
}
interface AccountView {
  id: string;
  usage: UsageSnapshot | null;
}

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
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

async function init() {
  const titleEl = $("p-title");
  const msgEl = $("p-msg");
  const statusEl = $("p-status");
  const timeRow = $("p-time-row");
  const timeEl = $<HTMLInputElement>("p-time");
  const primary = $<HTMLButtonElement>("p-primary");
  const secondary = $<HTMLButtonElement>("p-secondary");
  const win = getCurrentWindow();
  const close = () => win.close();

  let prompt = await invoke<SwitchPrompt | null>("get_pending_prompt");

  // существующие подписи ресета (для предзаполнения поля)
  const labelById = new Map<string, string>();
  for (const a of await invoke<AccountView[]>("list_accounts")) {
    if (a.usage?.reset_label) labelById.set(a.id, a.usage.reset_label);
  }

  // Шаг «после переключения»: задать время ресета аккаунта.
  function showSetReset(id: string, name: string, lead: string) {
    titleEl.textContent = "Время ресета";
    msgEl.textContent = `${lead} Когда сбрасывается лимит сессии «${name}»?`;
    statusEl.textContent = "";
    timeRow.hidden = false;
    timeEl.value = labelById.get(id) ?? "";
    primary.textContent = "Сохранить";
    secondary.hidden = false;
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
      secondary.hidden = false;
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

  if (!prompt) {
    // открыли диалог без запроса — закрыть, не залипая
    close();
    return;
  }
  route(prompt);
}

window.addEventListener("DOMContentLoaded", init);
