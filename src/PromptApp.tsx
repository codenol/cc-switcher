import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";

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

type View =
  // ресет уходящего аккаунта перед свапом
  | { kind: "beforeSwitch"; prompt: SwitchPrompt; outId: string; outName: string }
  // идёт переключение (кнопки заблокированы)
  | { kind: "switching"; title: string; message: string }
  // задать время ресета аккаунта (после свапа или для уже активного)
  | { kind: "setReset"; id: string; name: string; lead: string }
  // ошибка переключения
  | { kind: "error"; message: string };

export default function PromptApp() {
  const [view, setView] = useState<View | null>(null);
  const [time, setTime] = useState("");
  const [status, setStatus] = useState("");
  const labels = useRef<Map<string, string>>(new Map());
  const promptRef = useRef<SwitchPrompt | null>(null);
  const started = useRef(false);

  const close = useCallback(() => getCurrentWindow().close(), []);

  const runSwitch = useCallback((p: SwitchPrompt) => {
    setStatus("Завершаю Claude…");
    setView({
      kind: "switching",
      title: "Переключение",
      message: `Переключаюсь на «${p.target_name}»…`,
    });
    invoke("switch_account", { id: p.target_id });
  }, []);

  // Выбрать экран под запрос.
  const route = useCallback(
    (p: SwitchPrompt) => {
      promptRef.current = p;
      if (p.already_active) {
        setTime(labels.current.get(p.target_id) ?? "");
        setView({
          kind: "setReset",
          id: p.target_id,
          name: p.target_name,
          lead: "Аккаунт уже активен.",
        });
      } else if (p.outgoing_id && p.outgoing_name) {
        setTime(labels.current.get(p.outgoing_id) ?? "");
        setView({
          kind: "beforeSwitch",
          prompt: p,
          outId: p.outgoing_id,
          outName: p.outgoing_name,
        });
      } else {
        runSwitch(p);
      }
    },
    [runSwitch]
  );

  useEffect(() => {
    if (started.current) return;
    started.current = true;

    const unlisteners: Array<() => void> = [];

    (async () => {
      // подписи ресета для предзаполнения
      const accounts = await invoke<AccountView[]>("list_accounts");
      for (const a of accounts) {
        if (a.usage?.reset_label) labels.current.set(a.id, a.usage.reset_label);
      }

      unlisteners.push(
        await listen<string>("switch-progress", (e) => setStatus(e.payload))
      );
      unlisteners.push(
        await listen("switch-done", () => {
          const p = promptRef.current;
          if (!p) return;
          setTime(labels.current.get(p.target_id) ?? "");
          setView({
            kind: "setReset",
            id: p.target_id,
            name: p.target_name,
            lead: "Готово, сессия запущена.",
          });
        })
      );
      unlisteners.push(
        await listen<string>("switch-error", (e) => {
          setStatus(String(e.payload));
          setView({ kind: "error", message: "Переключение не удалось." });
        })
      );
      unlisteners.push(
        await listen("prompt-show", async () => {
          const fresh = await invoke<SwitchPrompt | null>("get_pending_prompt");
          if (fresh) route(fresh);
        })
      );

      const prompt = await invoke<SwitchPrompt | null>("get_pending_prompt");
      if (!prompt) {
        close();
        return;
      }
      route(prompt);
    })();

    return () => unlisteners.forEach((u) => u());
  }, [route, close]);

  async function saveReset(id: string) {
    const r = computeReset(time);
    if (r) await invoke("set_reset_time", { id, resetAt: r.reset_at, label: r.label });
  }

  if (!view) {
    return <div className="h-screen" />;
  }

  const showTime = view.kind === "beforeSwitch" || view.kind === "setReset";

  const title =
    view.kind === "beforeSwitch"
      ? "Перед переключением"
      : view.kind === "setReset"
        ? "Время ресета"
        : view.kind === "error"
          ? "Не получилось"
          : view.title;

  const message =
    view.kind === "beforeSwitch"
      ? `Когда сбрасывается лимит у «${view.outName}»?`
      : view.kind === "setReset"
        ? `${view.lead} Когда сбрасывается лимит сессии «${view.name}»?`
        : view.kind === "error"
          ? view.message
          : view.message;

  return (
    <div className="flex h-screen items-center justify-center p-4">
      <Card className="w-full max-w-[380px]">
        <CardHeader>
          <CardTitle>{title}</CardTitle>
          <CardDescription>{message}</CardDescription>
        </CardHeader>

        {(showTime || status) && (
          <CardContent className="flex flex-col gap-3">
            {showTime && (
              <div className="flex items-center justify-between gap-3">
                <span className="text-xs text-muted-foreground">
                  Время ресета
                </span>
                <Input
                  type="time"
                  autoFocus
                  value={time}
                  onChange={(e) => setTime(e.target.value)}
                  className="h-10 w-[120px] text-lg"
                />
              </div>
            )}
            {status && (
              <p className="text-xs text-muted-foreground/70">{status}</p>
            )}
          </CardContent>
        )}

        <CardFooter className="justify-end gap-2">
          {view.kind === "beforeSwitch" && (
            <>
              <Button variant="outline" onClick={() => runSwitch(view.prompt)}>
                Без времени
              </Button>
              <Button
                onClick={async () => {
                  await saveReset(view.outId);
                  runSwitch(view.prompt);
                }}
              >
                Сохранить и переключить
              </Button>
            </>
          )}

          {view.kind === "setReset" && (
            <>
              <Button variant="outline" onClick={close}>
                Пропустить
              </Button>
              <Button
                onClick={async () => {
                  await saveReset(view.id);
                  close();
                }}
              >
                Сохранить
              </Button>
            </>
          )}

          {view.kind === "switching" && <Button disabled>Переключаю…</Button>}

          {view.kind === "error" && <Button onClick={close}>Закрыть</Button>}
        </CardFooter>
      </Card>
    </div>
  );
}
