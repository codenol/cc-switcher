import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";
import { Plus } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Badge } from "@/components/ui/badge";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";

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

const NEW = "__new__";

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

// Chromium WebKit-время cookie (микросекунды от 1601) → дата + остаток дней.
function cookieExpiry(t: number | null): { date: string; days: number } | null {
  if (!t) return null;
  const ms = t / 1000 - 11644473600000;
  const date = new Date(ms).toLocaleDateString("ru-RU", { dateStyle: "medium" });
  const days = Math.floor((ms - Date.now()) / 86400000);
  return { date, days };
}

export default function SettingsApp() {
  const [accounts, setAccounts] = useState<AccountView[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [resetTime, setResetTime] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const loaded = useRef(false);

  const isNew = selectedId === NEW;
  const account = useMemo(
    () => accounts.find((a) => a.id === selectedId) ?? null,
    [accounts, selectedId]
  );

  // Заполнить форму из выбранного аккаунта.
  const fillForm = useCallback((acc: AccountView | null) => {
    setName(acc?.display_name ?? "");
    setResetTime(acc?.usage?.reset_label ?? "");
    setError("");
  }, []);

  const load = useCallback(
    async (preferId?: string | null) => {
      const list = await invoke<AccountView[]>("list_accounts");
      setAccounts(list);
      setSelectedId((prev) => {
        const want = preferId !== undefined ? preferId : prev;
        if (want === NEW) return NEW;
        if (want && list.some((a) => a.id === want)) return want;
        const def = list.find((a) => a.is_active) ?? list[0];
        return def?.id ?? null;
      });
    },
    []
  );

  useEffect(() => {
    if (loaded.current) return;
    loaded.current = true;
    load();
  }, [load]);

  // Синхронизировать форму при смене выбранного аккаунта.
  useEffect(() => {
    if (isNew) {
      setName("");
      setResetTime("");
      setError("");
    } else {
      fillForm(account);
    }
  }, [selectedId, account, isNew, fillForm]);

  const dirty = isNew
    ? name.trim().length > 0
    : !!account &&
      (name.trim() !== account.display_name ||
        resetTime !== (account.usage?.reset_label ?? ""));

  function startNew() {
    setSelectedId(NEW);
  }

  async function save() {
    const trimmed = name.trim();
    if (!trimmed) {
      setError("Введите имя аккаунта.");
      return;
    }
    setBusy(true);
    try {
      const id = await invoke<string>("save_account", {
        input: { id: isNew ? null : selectedId, display_name: trimmed },
      });
      const r = computeReset(resetTime);
      if (r) {
        await invoke("set_reset_time", { id, resetAt: r.reset_at, label: r.label });
      } else {
        await invoke("clear_reset_time", { id });
      }
      await load(id);
      toast.success("Сохранено");
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function remove() {
    if (!account) return;
    setBusy(true);
    try {
      await invoke("delete_account", { id: account.id });
      await load(null);
      toast.success("Аккаунт удалён");
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function capture() {
    if (isNew || !selectedId) {
      setError("Сначала сохраните аккаунт, потом захватывайте cookie.");
      return;
    }
    setBusy(true);
    try {
      await invoke("capture_account", { id: selectedId });
      await load(selectedId);
      toast.success("Cookie захвачены");
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  const showEmpty = accounts.length === 0 && !isNew;

  return (
    <div className="flex h-screen flex-col">
      {/* Строка аккаунтов: табы (≤4) + добавить */}
      {!showEmpty && (
        <header className="flex items-center gap-2 border-b border-border px-3 py-2">
          <Tabs
            value={account?.id ?? ""}
            onValueChange={(v) => setSelectedId(v)}
            className="min-w-0"
          >
            {accounts.length > 0 && (
              <TabsList>
                {accounts.map((a) => (
                  <TabsTrigger key={a.id} value={a.id}>
                    <span
                      className={
                        "h-1.5 w-1.5 shrink-0 rounded-full " +
                        (a.has_cookies ? "bg-success" : "bg-muted-foreground/50")
                      }
                      title={a.has_cookies ? "cookie захвачены" : "cookie не захвачены"}
                    />
                    {a.display_name}
                  </TabsTrigger>
                ))}
              </TabsList>
            )}
          </Tabs>
          <Button
            variant="outline"
            size="sm"
            className="border-dashed"
            onClick={startNew}
            data-state={isNew ? "active" : undefined}
          >
            <Plus className="h-3.5 w-3.5" />
            Аккаунт
          </Button>
        </header>
      )}

      {/* Пустое состояние */}
      {showEmpty && (
        <div className="flex flex-1 flex-col items-center justify-center gap-4 text-center">
          <p className="text-sm text-muted-foreground">Аккаунтов пока нет.</p>
          <Button size="lg" onClick={startNew}>
            Добавить первый аккаунт
          </Button>
        </div>
      )}

      {/* Форма редактирования */}
      {!showEmpty && (
        <main className="flex-1 overflow-y-auto px-6 py-5">
          <div className="mx-auto flex max-w-[480px] flex-col gap-4">
            {/* Статус-строка: активность + cookie + перезахват */}
            <div className="flex min-h-7 flex-wrap items-center gap-2">
              {account?.is_active && (
                <Badge variant="success">● Активен в Claude</Badge>
              )}
              {isNew ? (
                <span className="text-xs text-warning">
                  Новый аккаунт — сохраните, затем захватите cookie.
                </span>
              ) : (
                <CookieBadge account={account} />
              )}
              <Button
                variant="outline"
                size="sm"
                className="ml-auto"
                onClick={capture}
                disabled={busy || isNew}
              >
                {account?.has_cookies ? "Перезахватить" : "Захватить cookie"}
              </Button>
            </div>

            {/* Имя + время ресета в один ряд */}
            <div className="flex flex-wrap items-start gap-3">
              <div className="flex min-w-[200px] flex-1 flex-col gap-1.5">
                <Label htmlFor="f-name">Имя</Label>
                <Input
                  id="f-name"
                  value={name}
                  autoFocus={isNew}
                  placeholder="Например, Аня (дневная смена)"
                  onChange={(e) => setName(e.target.value)}
                  onKeyDown={(e) => e.key === "Enter" && dirty && !busy && save()}
                />
              </div>
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="f-reset">Ресет сессии</Label>
                <Input
                  id="f-reset"
                  type="time"
                  className="w-[130px]"
                  value={resetTime}
                  onChange={(e) => setResetTime(e.target.value)}
                />
                <p className="text-[11px] text-muted-foreground">
                  {resetTime
                    ? `лимит обнуляется в ${resetTime}`
                    : "необязательно"}
                </p>
              </div>
            </div>

            {error && <p className="text-xs text-destructive">{error}</p>}

            {/* Действия */}
            <div className="flex items-center gap-2 border-t border-border pt-4">
              <Button onClick={save} disabled={!dirty || busy}>
                Сохранить
              </Button>
              {!isNew && account && (
                <AlertDialog>
                  <AlertDialogTrigger asChild>
                    <Button variant="ghostDanger" className="ml-auto" disabled={busy}>
                      Удалить аккаунт
                    </Button>
                  </AlertDialogTrigger>
                  <AlertDialogContent>
                    <AlertDialogHeader>
                      <AlertDialogTitle>
                        Удалить «{account.display_name}»?
                      </AlertDialogTitle>
                      <AlertDialogDescription>
                        Аккаунт и захваченные cookie будут удалены без возможности
                        восстановления.
                      </AlertDialogDescription>
                    </AlertDialogHeader>
                    <AlertDialogFooter>
                      <AlertDialogCancel>Отмена</AlertDialogCancel>
                      <AlertDialogAction onClick={remove}>Удалить</AlertDialogAction>
                    </AlertDialogFooter>
                  </AlertDialogContent>
                </AlertDialog>
              )}
            </div>
          </div>
        </main>
      )}
    </div>
  );
}

function CookieBadge({ account }: { account: AccountView | null }) {
  if (!account?.has_cookies) {
    return <Badge variant="warning">● Cookie не захвачены</Badge>;
  }
  const exp = cookieExpiry(account.session_expires_utc);
  if (!exp) return <Badge variant="success">✓ Cookie захвачены</Badge>;
  const variant = exp.days < 0 ? "destructive" : exp.days < 7 ? "warning" : "success";
  const text =
    exp.days < 0 ? `Cookie истекли ${exp.date}` : `✓ Cookie до ${exp.date}`;
  return <Badge variant={variant}>{text}</Badge>;
}
