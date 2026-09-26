// エフェクトの名前・色・アイコン(ミキサーの列・ノード表示・インスペクターで共通)。
import type { IconName } from "./icons";
import type { EffectView, ProjectEffect } from "./types";

/** エフェクトのプリセットの棚からノード表示へドラッグするときのデータの種類(中身はプリセット名) */
export const FX_PRESET_MIME = "application/x-glaux-fx-preset";

/** 種類ごとの色(カードの帯・列の印) */
export const FX_COLORS: Record<string, string> = {
  eq: "#4f8fdd",
  compressor: "#9aa3ad",
  distortion: "#e07a2e",
  amp: "#e05555",
  reverb: "#a47ae0",
  delay: "#3fbf9f",
  chorus: "#38a9d6",
  tape: "#c08a55",
  sidechain: "#caa43a",
  clap: "#b07ce8",
};

/** 種類の日本語名 */
export const FX_KIND_JA: Record<string, string> = {
  eq: "EQ",
  compressor: "コンプ",
  distortion: "歪み",
  amp: "アンプ",
  reverb: "リバーブ",
  delay: "ディレイ",
  chorus: "コーラス",
  tape: "テープ",
  sidechain: "サイドチェイン",
};

/** 種類のキー(内蔵エフェクト名。CLAP は "clap") */
export function fxKind(e: EffectView | ProjectEffect): string {
  if ("type" in e && e.type === "clap") return "clap";
  if (e.name === "clap") return "clap";
  return e.name ?? "effect";
}

export function fxColor(e: EffectView | ProjectEffect): string {
  return FX_COLORS[fxKind(e)] ?? "#777";
}

export function fxIcon(e: EffectView | ProjectEffect): IconName {
  return fxKind(e) === "clap" ? "plug" : "sliders-horizontal";
}

/** 表示名: ユーザーが付けた名前 → CLAP はプラグイン名 → 内蔵は種類の名前 */
export function fxName(e: EffectView | ProjectEffect, clapNames?: Map<string, string>): string {
  if (e.label) return e.label;
  if (fxKind(e) === "clap") {
    const view = e as EffectView;
    return view.plugin_name ?? clapNames?.get(e.plugin_id ?? "") ?? e.plugin_id?.split(".").pop() ?? "CLAP";
  }
  return e.name ?? "effect";
}

/** 並べ替えの move_effect に渡す位置。線の並び(外してあるものを除く)の中で `chainTo` 番目にしたいとき、
 *  全体の並び(外してあるものも含む)での位置を返す。変わらなければ null */
export function moveIndexFor(all: { id: string; parked?: boolean }[], id: string, chainTo: number): number | null {
  const without = all.filter((e) => e.id !== id);
  const chain = without.filter((e) => !e.parked);
  const from = all.findIndex((e) => e.id === id);
  let to: number;
  if (chainTo < chain.length) {
    // 並びで次に来るものの前へ
    to = without.findIndex((e) => e.id === chain[chainTo].id);
  } else if (chain.length > 0) {
    // 末尾: 並びの最後のものの後ろへ
    to = without.findIndex((e) => e.id === chain[chain.length - 1].id) + 1;
  } else {
    to = 0;
  }
  return to === from ? null : to;
}

/** 線の並びの `chainTo` 番目に新しく入れるとき、全体の並び(外してあるものも含む)での位置 */
export function insertIndexFor(all: { id: string; parked?: boolean }[], chainTo: number): number {
  const chain = all.filter((e) => !e.parked);
  if (chainTo < chain.length) return all.findIndex((e) => e.id === chain[chainTo].id);
  if (chain.length > 0) return all.findIndex((e) => e.id === chain[chain.length - 1].id) + 1;
  return 0;
}
