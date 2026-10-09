// 音色エディタの小さな道具: 要素の組み立て・アイコン・canvas の描き方・音名。
// 音色エディタの画面は、試作(66 版で了承)の組み立てをそのまま移すため、Svelte の部品ではなく要素を直に組み立てる
// (canvas の絵が中心で、描き直しの頻度が高い)。外側の枠(見出し・下の帯)は SoundEditor.svelte
import { SE_ICONS } from "./icons";

type Attr = string | number | boolean | null | undefined | ((ev: any) => void);
type Kid = Node | string | number | null | undefined | false | Kid[];

/** 要素を作る。on◯◯ はイベント、class・style はそのまま、true は属性だけ、false・null は付けない */
export function el<K extends keyof HTMLElementTagNameMap>(tag: K, attrs: Record<string, Attr> = {}, ...kids: Kid[]): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") e.className = String(v ?? "");
    else if (k === "style") e.style.cssText = String(v ?? "");
    else if (k.startsWith("on") && typeof v === "function") e.addEventListener(k.slice(2), v as EventListener);
    else if (v === true) e.setAttribute(k, "");
    else if (v !== false && v != null) e.setAttribute(k, String(v));
  }
  const add = (k: Kid) => {
    if (Array.isArray(k)) k.forEach(add);
    else if (k != null && k !== false) e.append(typeof k === "object" ? k : document.createTextNode(String(k)));
  };
  kids.forEach(add);
  return e;
}

export const clamp = (v: number, a: number, b: number) => Math.max(a, Math.min(b, v));

/** 線画のアイコン(道具・加工・つまみ) */
export function ico(k: string): HTMLSpanElement {
  const s = el("span", { class: "ico" });
  s.innerHTML = `<svg viewBox="0 0 16 16">${SE_ICONS[k] ?? ""}</svg>`;
  return s;
}

/** 音色エディタの一番外の要素(色の変数を読む所) */
let root: HTMLElement | null = null;
export function setRoot(r: HTMLElement | null) {
  root = r;
}
export const byId = <T extends HTMLElement = HTMLElement>(id: string): T | null => (root?.querySelector(`#${CSS.escape(id)}`) as T | null) ?? null;
/** 色などの CSS の変数(音色エディタの中の値) */
export const css = (n: string) => getComputedStyle(root ?? document.documentElement).getPropertyValue(n).trim();

/** canvas を表示の大きさ × 画素比に合わせて、描く前の 2D の入れ物を返す */
export function ctx2d(c: HTMLCanvasElement): { g: CanvasRenderingContext2D; w: number; h: number } {
  const r = c.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  const w = Math.max(1, Math.round(r.width * dpr)),
    h = Math.max(1, Math.round(r.height * dpr));
  if (c.width !== w || c.height !== h) {
    c.width = w;
    c.height = h;
  }
  const g = c.getContext("2d")!;
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.clearRect(0, 0, r.width, r.height);
  g.lineWidth = 1;
  g.setLineDash([]);
  g.globalAlpha = 1;
  g.textAlign = "left";
  g.lineCap = "butt";
  g.lineJoin = "miter";
  return { g, w: r.width, h: r.height };
}

/** 素材(音声)の波形を描く。x は -1〜1 の配列 */
export function drawAudio(g: CanvasRenderingContext2D, x: ArrayLike<number>, w: number, top: number, bot: number, color = "#7fd3cc") {
  const mid = (top + bot) / 2,
    amp = (bot - top) / 2 - 4;
  g.strokeStyle = color;
  g.lineWidth = 1;
  g.beginPath();
  for (let px = 0; px < w; px++) {
    const i0 = Math.floor((px / w) * x.length),
      i1 = Math.max(i0 + 1, Math.floor(((px + 1) / w) * x.length));
    let mn = 0,
      mx = 0;
    for (let i = i0; i < i1 && i < x.length; i++) {
      mn = Math.min(mn, x[i]);
      mx = Math.max(mx, x[i]);
    }
    g.moveTo(px + 0.5, mid - mx * amp);
    g.lineTo(px + 0.5, mid - mn * amp);
  }
  g.stroke();
}

/** 時間の物差し(秒)。x0〜x1 が 0〜len 秒。目盛りの間隔は幅に合わせて選ぶ */
export function timeRuler(g: CanvasRenderingContext2D, x0: number, x1: number, y: number, len: number, secLabel = " 秒") {
  const steps = [0.05, 0.1, 0.25, 0.5, 1, 2, 5];
  const step = steps.find((st) => ((x1 - x0) / len) * st >= 46) ?? 5;
  g.strokeStyle = "#3a3a3a";
  g.fillStyle = css("--faint");
  g.font = "9.5px sans-serif";
  g.lineWidth = 1;
  g.beginPath();
  g.moveTo(x0, y);
  g.lineTo(x1, y);
  g.stroke();
  let lastEnd = -1e9;
  for (let t = 0; t <= len + 1e-9; t += step) {
    const x = x0 + (t / len) * (x1 - x0),
      label = `${+t.toFixed(2)}${t === 0 ? secLabel : ""}`,
      lx = Math.min(x + 2, x1 - g.measureText(label).width - 2);
    g.beginPath();
    g.moveTo(x, y);
    g.lineTo(x, y + 4);
    g.stroke();
    // 数字がくっつく所は省く
    if (lx > lastEnd + 4) {
      g.fillText(label, lx, y + 12);
      lastEnd = lx + g.measureText(label).width;
    }
  }
}

/** 「音域の高い側では聞こえない」棒の塗り: その棒の色と暗い色の「\」向きの縞(「鳴らさない」の網掛け「/」と分ける) */
export function stripe(g: CanvasRenderingContext2D, color: string): CanvasPattern | string {
  const c = document.createElement("canvas");
  c.width = c.height = 4;
  const p = c.getContext("2d");
  if (!p) return color;
  p.fillStyle = "#16201f";
  p.fillRect(0, 0, 4, 4);
  p.strokeStyle = color;
  p.lineWidth = 1.6;
  p.beginPath();
  p.moveTo(-1, -1);
  p.lineTo(5, 5);
  p.moveTo(-1, 3);
  p.lineTo(1, 5);
  p.moveTo(3, -1);
  p.lineTo(5, 1);
  p.stroke();
  return g.createPattern(c, "repeat") ?? color;
}

/** 目盛りの字: 絵の左右の端で切れないよう、はみ出す分だけ内側へ寄せて描く */
export function axisText(g: CanvasRenderingContext2D, text: string, x: number, y: number, w: number) {
  const tw = g.measureText(text).width;
  g.fillText(text, clamp(x, 1, w - tw - 1), y);
}

/** 横に並ぶ目盛りの字: 端で切れないよう内側へ寄せ、くっつく字は間引く(両端の字を優先して残す)。items = [[字, 左の x], …](左から順) */
export function axisRow(g: CanvasRenderingContext2D, items: [string, number][], y: number, w: number, gap = 5) {
  const pos = items.map(([t, x]) => {
    const tw = g.measureText(t).width,
      l = clamp(x, 1, w - tw - 1);
    return { t, l, r: l + tw };
  });
  if (!pos.length) return;
  const keep = [pos[0]];
  const lastP = pos[pos.length - 1];
  if (pos.length > 1 && lastP.l >= pos[0].r + gap) keep.push(lastP);
  pos.slice(1, -1).forEach((p) => {
    if (keep.every((q) => p.l >= q.r + gap || p.r + gap <= q.l)) keep.push(p);
  });
  keep.forEach((p) => g.fillText(p.t, p.l, y));
}

/** 名札(黒い下地に色の字)。描いた札の箱 [左, 上, 右, 下] を返す(ほかの札を避けるのに使う) */
export function tag(g: CanvasRenderingContext2D, text: string, x: number, y: number, w: number, color = "#fff"): [number, number, number, number] {
  g.font = "11px sans-serif";
  const tw = g.measureText(text).width,
    lx = clamp(x, 2, w - tw - 8);
  g.fillStyle = "rgba(0,0,0,0.78)";
  g.fillRect(lx - 3, y - 11, tw + 6, 15);
  g.fillStyle = color;
  g.fillText(text, lx, y);
  return [lx - 3, y - 11, lx + tw + 3, y + 4];
}

// ---- 音名・周波数 ----
const NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
/** 60 = C4。負の数でも音名にする */
export const noteName = (m: number) => NOTE_NAMES[((m % 12) + 12) % 12] + (Math.floor(m / 12) - 1);
/** 同じなら「C4」、違えば「C4〜E4」 */
export const noteSpan = (a: number, b: number) => (a === b ? noteName(a) : `${noteName(a)}〜${noteName(b)}`);
export const midiHz = (m: number) => 440 * Math.pow(2, (m - 69) / 12);
/** Hz の範囲の表し方(同じなら 1 つ) */
export const hzRange = (a: number, b: number) => {
  const f = (x: number) => (x >= 1000 ? `${(x / 1000).toFixed(x >= 10000 ? 1 : 2)}k` : `${Math.round(x)}`);
  return f(a) === f(b) ? `${f(a)} Hz` : `${f(a)}〜${f(b)} Hz`;
};
/** 秒の表し方(0.1 秒未満は ms) */
export const fmtS = (v: number) => (v < 0.1 ? `${Math.round(v * 1000)} ms` : `${v.toFixed(2)} s`);
/** 包絡の値(押してから t 秒)。{a, d, s} は秒・0〜1 */
export const envAt = (e: { a: number; d: number; s: number }, t: number) =>
  t < e.a ? t / Math.max(e.a, 1e-4) : e.s + (1 - e.s) * Math.exp(-(t - e.a) / Math.max(e.d / 3, 1e-3));
/** 決まった並びの乱数(絵がちらつかないように) */
export const seeded = (seed: number) => () => ((seed = (seed * 16807) % 2147483647) - 1) / 2147483646;
/** 写し(JSON で表せる値だけ) */
export const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v));
