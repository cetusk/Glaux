// 画面の隅に出す短い知らせ(トースト)。失敗・警告・成功を同じ見た目で出す。
// 以前は、編集の失敗を黙って捨てる箇所(.catch(() => {}))と alert() が混在していた。

export type ToastKind = "error" | "warn" | "ok";

export interface Toast {
  id: number;
  kind: ToastKind;
  text: string;
}

export const toasts = $state<{ list: Toast[] }>({ list: [] });

let nextId = 1;
/** 同じ文面が続けて来たときは重ねない(連続した失敗で画面が埋まらないように) */
const DEDUP_MS = 1500;
let last: { text: string; at: number } | null = null;

/** 知らせを出す。error は 8 秒、それ以外は 4 秒で消える(× で先に消せる) */
export function showToast(kind: ToastKind, text: string) {
  const now = Date.now();
  if (last && last.text === text && now - last.at < DEDUP_MS) return;
  last = { text, at: now };
  const id = nextId++;
  toasts.list.push({ id, kind, text });
  if (toasts.list.length > 4) toasts.list.shift();
  setTimeout(() => dismissToast(id), kind === "error" ? 8000 : 4000);
}

export function dismissToast(id: number) {
  const i = toasts.list.findIndex((t) => t.id === id);
  if (i >= 0) toasts.list.splice(i, 1);
}

/** 失敗(例外)を知らせる。`what` は何をしようとしていたか */
export function showError(what: string, e: unknown) {
  showToast("error", `${what}: ${String(e).replace(/^Error: /, "")}`);
}
