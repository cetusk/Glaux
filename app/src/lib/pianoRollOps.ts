// ピアノロールの定数と純粋な計算(奏法の表・スナップの選択肢・音名・ピッチカーブの間引き・描画の色)
import type { Articulation } from "./types";

/// ブラウザの Canvas 実サイズ上限(超えると描画が黙って全部消える)。
/// 長いクリップ × ズームで超えうるので、上限内に収まる解像度スケールに落とす
/// (見た目は CSS サイズのまま。極端な場合だけ少しぼやける)
export const MAX_CANVAS_PX = 15000;

export type ArtEntry = { art: Articulation; key: string; label: string };

// この楽器で効く奏法(glaux-dsp params.rs の articulations_for と同期を保つこと)
export const ARTS_BY_INSTRUMENT: Record<string, ArtEntry[]> = {
  subtractive: [
    { art: "palm_mute", key: "M", label: "ミュート" },
    { art: "staccato", key: "S", label: "スタッカート" },
    { art: "accent", key: "A", label: "アクセント" },
    { art: "vibrato", key: "V", label: "ビブラート" },
    { art: "bend", key: "B", label: "チョーキング" },
    { art: "legato", key: "T", label: "レガート" },
    { art: "portamento", key: "P", label: "ポルタメント" },
  ],
  drum: [{ art: "accent", key: "A", label: "アクセント" }],
  pluck: [
    { art: "palm_mute", key: "M", label: "ブリッジミュート" },
    { art: "staccato", key: "S", label: "スタッカート" },
    { art: "accent", key: "A", label: "アクセント" },
    { art: "vibrato", key: "V", label: "ビブラート" },
    { art: "bend", key: "B", label: "チョーキング" },
    { art: "legato", key: "T", label: "ハンマリング" },
    { art: "portamento", key: "P", label: "スライド" },
  ],
  sampler: [
    { art: "staccato", key: "S", label: "スタッカート" },
    { art: "accent", key: "A", label: "アクセント" },
    { art: "vibrato", key: "V", label: "ビブラート" },
    { art: "bend", key: "B", label: "チョーキング" },
    { art: "legato", key: "T", label: "レガート" },
    { art: "portamento", key: "P", label: "ポルタメント" },
  ],
  sf2: [
    { art: "staccato", key: "S", label: "スタッカート" },
    { art: "accent", key: "A", label: "アクセント" },
    { art: "vibrato", key: "V", label: "ビブラート" },
    { art: "bend", key: "B", label: "チョーキング" },
    { art: "legato", key: "T", label: "レガート" },
    { art: "portamento", key: "P", label: "ポルタメント" },
  ],
  fm: [
    { art: "palm_mute", key: "M", label: "ミュート" },
    { art: "staccato", key: "S", label: "スタッカート" },
    { art: "accent", key: "A", label: "アクセント" },
    { art: "vibrato", key: "V", label: "ビブラート" },
    { art: "bend", key: "B", label: "チョーキング" },
    { art: "legato", key: "T", label: "レガート" },
    { art: "portamento", key: "P", label: "ポルタメント" },
  ],
  wavetable: [
    { art: "palm_mute", key: "M", label: "ミュート" },
    { art: "staccato", key: "S", label: "スタッカート" },
    { art: "accent", key: "A", label: "アクセント" },
    { art: "vibrato", key: "V", label: "ビブラート" },
    { art: "bend", key: "B", label: "チョーキング" },
    { art: "legato", key: "T", label: "レガート" },
    { art: "portamento", key: "P", label: "ポルタメント" },
  ],
  // CLAP 音源: ビブラート・ベンドは音程の変化として送り、ミュート・アクセントは長さと強さで近づける
  clap: [
    { art: "palm_mute", key: "M", label: "ミュート(短く弱く)" },
    { art: "staccato", key: "S", label: "スタッカート" },
    { art: "accent", key: "A", label: "アクセント" },
    { art: "vibrato", key: "V", label: "ビブラート" },
    { art: "bend", key: "B", label: "チョーキング" },
    { art: "legato", key: "T", label: "レガート(重ねて送る)" },
    { art: "portamento", key: "P", label: "ポルタメント" },
  ],
};

export const ART_LABELS: Record<Articulation, string> = {
  normal: "通常",
  palm_mute: "ブリッジミュート",
  staccato: "スタッカート",
  accent: "アクセント",
  vibrato: "ビブラート",
  bend: "チョーキング",
  legato: "レガート",
  portamento: "ポルタメント",
};

// T = 3 連符(PPQ 960: 1/4T=640, 1/8T=320, 1/16T=160)
export const SNAP_OPTIONS = [
  { label: "1 小節", ticks: 3840 },
  { label: "1/2", ticks: 1920 },
  { label: "1/4", ticks: 960 },
  { label: "1/4T", ticks: 640 },
  { label: "1/8", ticks: 480 },
  { label: "1/8T", ticks: 320 },
  { label: "1/16", ticks: 240 },
  { label: "1/16T", ticks: 160 },
];

/// ポルタメントの滑る時間の選択肢(ms)
export const GLIDE_CHOICES = [40, 80, 150, 250, 400, 800];

const NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
export const BLACK = new Set([1, 3, 6, 8, 10]);

export function noteName(pitch: number): string {
  return `${NOTE_NAMES[pitch % 12]}${Math.floor(pitch / 12) - 1}`;
}

const MAX_CURVE_POINTS = 8;

/// なぞった軌跡を最大 8 点に間引く(時間方向に等間隔で取り、線形補間で値を読む)
export function simplifyCurve(pts: { t: number; c: number }[]): { tick: number; cents: number }[] {
  const sorted = [...pts].sort((a, b) => a.t - b.t);
  if (sorted.length === 0) return [];
  const t0 = sorted[0].t;
  const t1 = sorted[sorted.length - 1].t;
  const valueAt = (t: number) => {
    let i = sorted.findIndex((p) => p.t >= t);
    if (i <= 0) return sorted[Math.max(0, i)].c;
    const a = sorted[i - 1];
    const b = sorted[i];
    const f = b.t === a.t ? 0 : (t - a.t) / (b.t - a.t);
    return a.c + (b.c - a.c) * f;
  };
  const n = t1 - t0 < 1 ? 1 : MAX_CURVE_POINTS;
  const out: { tick: number; cents: number }[] = [];
  for (let k = 0; k < n; k++) {
    const t = n === 1 ? t0 : t0 + ((t1 - t0) * k) / (n - 1);
    const tick = Math.round(t);
    if (out.length > 0 && out[out.length - 1].tick === tick) continue;
    out.push({ tick, cents: Math.round(valueAt(t)) });
  }
  return out;
}

/// AI の編集の色(CSS の --ai)
export function aiColor(): string {
  return getComputedStyle(document.documentElement).getPropertyValue("--ai").trim() || "#b07ce8";
}

/// テーマのアクセント色("r, g, b")。選択・再生ヘッド・ホバーをタイムラインと同じ色で描く
/// (以前は琥珀色の固定値で、テーマを変えてもピアノロールだけ色が変わらなかった)
let accentCache = { css: "", rgb: "255, 194, 71" };
export function accentRgb(): string {
  const css = getComputedStyle(document.documentElement).getPropertyValue("--accent").trim();
  if (css === accentCache.css) return accentCache.rgb;
  const m = css.match(/^#([0-9a-f]{3}|[0-9a-f]{6})$/i);
  let rgb = accentCache.rgb;
  if (m) {
    const h = m[1].length === 3 ? [...m[1]].map((c) => c + c).join("") : m[1];
    rgb = [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16)).join(", ");
  }
  accentCache = { css, rgb };
  return rgb;
}
