// ピアノロールの定数と純粋な計算(奏法の表・スナップの選択肢・音名・ピッチカーブの間引き・描画の色)
import { tr } from "./i18n.svelte";
import type { Articulation } from "./types";

/// ブラウザの Canvas 実サイズ上限(超えると描画が黙って全部消える)。
/// 長いクリップ × ズームで超えうるので、上限内に収まる解像度スケールに落とす
/// (見た目は CSS サイズのまま。極端な場合だけ少しぼやける)
export const MAX_CANVAS_PX = 15000;

export type ArtEntry = { art: Articulation; key: string; label: string };

/// 奏法の表の 1 行。表示名(label)は読むたびに今の言語で選ぶ(言語を切り替えたら描き直される)
function artEntry(art: Articulation, key: string, ja: string, en: string): ArtEntry {
  return {
    art,
    key,
    get label() {
      return tr(ja, en);
    },
  };
}

// この楽器で効く奏法(glaux-dsp params.rs の articulations_for と同期を保つこと)
export const ARTS_BY_INSTRUMENT: Record<string, ArtEntry[]> = {
  subtractive: [
    artEntry("palm_mute", "M", "ミュート", "Mute"),
    artEntry("staccato", "S", "スタッカート", "Staccato"),
    artEntry("accent", "A", "アクセント", "Accent"),
    artEntry("vibrato", "V", "ビブラート", "Vibrato"),
    artEntry("bend", "B", "チョーキング", "Bend"),
    artEntry("legato", "T", "レガート", "Legato"),
    artEntry("portamento", "P", "ポルタメント", "Portamento"),
  ],
  drum: [artEntry("accent", "A", "アクセント", "Accent")],
  pluck: [
    artEntry("palm_mute", "M", "ブリッジミュート", "Palm mute"),
    artEntry("staccato", "S", "スタッカート", "Staccato"),
    artEntry("accent", "A", "アクセント", "Accent"),
    artEntry("vibrato", "V", "ビブラート", "Vibrato"),
    artEntry("bend", "B", "チョーキング", "Bend"),
    artEntry("legato", "T", "ハンマリング", "Hammer-on"),
    artEntry("portamento", "P", "スライド", "Slide"),
  ],
  sampler: [
    artEntry("staccato", "S", "スタッカート", "Staccato"),
    artEntry("accent", "A", "アクセント", "Accent"),
    artEntry("vibrato", "V", "ビブラート", "Vibrato"),
    artEntry("bend", "B", "チョーキング", "Bend"),
    artEntry("legato", "T", "レガート", "Legato"),
    artEntry("portamento", "P", "ポルタメント", "Portamento"),
  ],
  sf2: [
    artEntry("staccato", "S", "スタッカート", "Staccato"),
    artEntry("accent", "A", "アクセント", "Accent"),
    artEntry("vibrato", "V", "ビブラート", "Vibrato"),
    artEntry("bend", "B", "チョーキング", "Bend"),
    artEntry("legato", "T", "レガート", "Legato"),
    artEntry("portamento", "P", "ポルタメント", "Portamento"),
  ],
  fm: [
    artEntry("palm_mute", "M", "ミュート", "Mute"),
    artEntry("staccato", "S", "スタッカート", "Staccato"),
    artEntry("accent", "A", "アクセント", "Accent"),
    artEntry("vibrato", "V", "ビブラート", "Vibrato"),
    artEntry("bend", "B", "チョーキング", "Bend"),
    artEntry("legato", "T", "レガート", "Legato"),
    artEntry("portamento", "P", "ポルタメント", "Portamento"),
  ],
  wavetable: [
    artEntry("palm_mute", "M", "ミュート", "Mute"),
    artEntry("staccato", "S", "スタッカート", "Staccato"),
    artEntry("accent", "A", "アクセント", "Accent"),
    artEntry("vibrato", "V", "ビブラート", "Vibrato"),
    artEntry("bend", "B", "チョーキング", "Bend"),
    artEntry("legato", "T", "レガート", "Legato"),
    artEntry("portamento", "P", "ポルタメント", "Portamento"),
  ],
  // CLAP 音源: ビブラート・ベンドは音程の変化として送り、ミュート・アクセントは長さと強さで近づける
  clap: [
    artEntry("palm_mute", "M", "ミュート(短く弱く)", "Mute (short and soft)"),
    artEntry("staccato", "S", "スタッカート", "Staccato"),
    artEntry("accent", "A", "アクセント", "Accent"),
    artEntry("vibrato", "V", "ビブラート", "Vibrato"),
    artEntry("bend", "B", "チョーキング", "Bend"),
    artEntry("legato", "T", "レガート(重ねて送る)", "Legato (overlapped)"),
    artEntry("portamento", "P", "ポルタメント", "Portamento"),
  ],
};

/// 奏法の表示名。値は読むたびに今の言語で選ぶ(getter)
export const ART_LABELS: Record<Articulation, string> = {
  get normal() {
    return tr("通常", "Normal");
  },
  get palm_mute() {
    return tr("ブリッジミュート", "Palm mute");
  },
  get staccato() {
    return tr("スタッカート", "Staccato");
  },
  get accent() {
    return tr("アクセント", "Accent");
  },
  get vibrato() {
    return tr("ビブラート", "Vibrato");
  },
  get bend() {
    return tr("チョーキング", "Bend");
  },
  get legato() {
    return tr("レガート", "Legato");
  },
  get portamento() {
    return tr("ポルタメント", "Portamento");
  },
};

// T = 3 連符(PPQ 960: 1/4T=640, 1/8T=320, 1/16T=160)
export const SNAP_OPTIONS = [
  {
    get label() {
      return tr("1 小節", "1 bar");
    },
    ticks: 3840,
  },
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
