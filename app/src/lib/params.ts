// つまみ(パラメータ)のスライダーの位置と値の変換、値の表示(インスペクターとノード表示で共通)。
import type { ParamView } from "./types";

export function fmtValue(p: ParamView, dragging?: number): string {
  // CLAP プラグインのつまみはプラグイン自身の表示を優先(ドラッグ中はその値)
  if (dragging === undefined && p.current_text) return p.current_text;
  const v = dragging ?? p.current;
  if (typeof v === "number") {
    const digits = p.range.kind === "int" ? 0 : Math.abs(v) >= 100 ? 0 : 2;
    return `${v.toFixed(digits)}${p.unit ?? ""}`;
  }
  return `${v}`;
}

// ---- スライダー: 位置(0〜SLIDER_MAX)と値の変換 ----
// 周波数・時間のように広い範囲を持つパラメータは skew(< 1)で下側の分解能を上げる
// (以前は線形で、カットオフ 40〜12000Hz のうち 200〜800Hz がスライダーの 5% しかなかった)。
// 式は JUCE と同じ: 位置 = ((値 − 最小) / 幅)^skew
export const SLIDER_MAX = 1000;

export function toPos(p: ParamView, v: number): number {
  if (p.range.kind !== "float" && p.range.kind !== "int") return 0;
  const { min, max } = p.range;
  const t = Math.min(1, Math.max(0, (v - min) / (max - min || 1)));
  const skew = p.range.kind === "float" ? (p.range.skew ?? 1) : 1;
  return Math.round(Math.pow(t, skew) * SLIDER_MAX);
}

export function fromPos(p: ParamView, pos: number): number {
  if (p.range.kind !== "float" && p.range.kind !== "int") return 0;
  const { min, max } = p.range;
  const skew = p.range.kind === "float" ? (p.range.skew ?? 1) : 1;
  const v = min + (max - min) * Math.pow(pos / SLIDER_MAX, 1 / skew);
  if (p.range.kind === "int") return Math.round(v);
  // 表示の桁に合わせて丸める(履歴に 0.30000000004 のような値を残さない)
  const digits = Math.abs(v) >= 100 ? 1 : 3;
  return Number(v.toFixed(digits));
}
