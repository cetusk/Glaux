// エフェクトの名前・色・アイコン(ミキサーの列・ノード表示・インスペクターで共通)。
import type { IconName } from "./icons";
import type { EffectView, FxLink, ProjectEffect } from "./types";

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
  multiband: "#7f9cc0",
  transient: "#d0739a",
  limiter: "#d9534f",
  width: "#5fb3c9",
  dynamic_eq: "#6f86e8",
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
  multiband: "マルチバンド",
  transient: "トランジェント",
  limiter: "リミッタ",
  width: "幅",
  dynamic_eq: "ダイナミック EQ",
  sidechain: "サイドチェイン",
};

/** 選択肢が空の「検出のトラック」(サイドチェイン・ダイナミック EQ の source)は、曲のトラックから選ばせる */
export function trackChoices(
  p: { name: string; range: { kind: string; choices?: readonly string[] } },
  tracks: { id: string; name: string }[],
): { value: string; label: string }[] | null {
  if (p.name !== "source" || p.range.kind !== "enum" || (p.range.choices?.length ?? 0) > 0) return null;
  return [{ value: "", label: "なし(自分の音)" }, ...tracks.map((t) => ({ value: t.id, label: t.name }))];
}

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

// ---- エフェクトのつながり(ノード表示の線)。glaux-core の model/routing.rs と同じ決まり ----

export const IN = "in";
export const OUT = "out";

export const linkKey = (l: FxLink) => `${l.from}>${l.to}`;

/** 実際に使う線。表が無ければ並び順の直列(外してある parked を除く) */
export function effectiveLinks(effects: { id: string; parked?: boolean }[], links: FxLink[] | null | undefined): FxLink[] {
  if (links) return links.map((l) => ({ ...l }));
  const out: FxLink[] = [];
  let prev = IN;
  for (const e of effects.filter((e) => !e.parked)) {
    out.push({ from: prev, to: e.id });
    prev = e.id;
  }
  out.push({ from: prev, to: OUT });
  return out;
}

function reach(links: FxLink[], start: string, forward: boolean): Set<string> {
  const seen = new Set([start]);
  const stack = [start];
  while (stack.length) {
    const n = stack.pop()!;
    for (const l of links) {
      const [a, b] = forward ? [l.from, l.to] : [l.to, l.from];
      if (a === n && !seen.has(b)) {
        seen.add(b);
        stack.push(b);
      }
    }
  }
  return seen;
}

/** 鳴るエフェクト(入力から出口まで線でたどれるもの) */
export function soundingSet(links: FxLink[]): Set<string> {
  const f = reach(links, IN, true);
  const b = reach(links, OUT, false);
  return new Set([...f].filter((x) => b.has(x) && x !== IN && x !== OUT));
}

/** 入力から来ているか(鳴らない理由の表示用) */
export const fromInput = (links: FxLink[]) => reach(links, IN, true);

/** 鳴るエフェクトを処理の順に(同じ段では並び順) */
export function processingOrder(effects: { id: string }[], links: FxLink[]): string[] {
  const on = soundingSet(links);
  const ids = effects.map((e) => e.id).filter((id) => on.has(id));
  const indeg = new Map(ids.map((id) => [id, 0]));
  for (const l of links) if (on.has(l.from) && indeg.has(l.to)) indeg.set(l.to, indeg.get(l.to)! + 1);
  const out: string[] = [];
  const done = new Set<string>();
  while (out.length < ids.length) {
    const next = ids.find((id) => !done.has(id) && indeg.get(id) === 0);
    if (!next) break;
    done.add(next);
    out.push(next);
    for (const l of links) if (l.from === next && indeg.has(l.to)) indeg.set(l.to, indeg.get(l.to)! - 1);
  }
  return out;
}

/** a → b をつなげるか。つなげなければ理由 */
export function connectProblem(links: FxLink[], a: string, b: string): string | null {
  if (a === b) return "自分自身にはつなげない";
  if (a === OUT || b === IN) return "向きが逆";
  if (links.some((l) => l.from === a && l.to === b)) return "もうつながっている";
  if (reach(links, b, true).has(a)) return "輪になるのでつなげない";
  return null;
}

/** 出口の直前に入れる(出口へ入っていた線をこのエフェクトへ付け替え、このエフェクト → 出口) */
export function insertBeforeOutput(links: FxLink[], id: string): FxLink[] {
  const out: FxLink[] = [];
  let redirected = false;
  for (const l of links) {
    if (l.to === OUT) {
      redirected = true;
      if (!out.some((x) => x.from === l.from && x.to === id)) out.push({ from: l.from, to: id, gain_db: l.gain_db });
    } else out.push(l);
  }
  if (!redirected) out.push({ from: IN, to: id });
  out.push({ from: id, to: OUT });
  return out;
}

/** 線を全部外して前後をつなぎ直す(a → X → b を a → b に。音量は足す) */
export function unlinkBridging(links: FxLink[], id: string): FxLink[] {
  const ins = links.filter((l) => l.to === id);
  const outs = links.filter((l) => l.from === id);
  const out = links.filter((l) => l.from !== id && l.to !== id);
  for (const a of ins)
    for (const b of outs) {
      if (a.from === b.to || out.some((x) => x.from === a.from && x.to === b.to)) continue;
      out.push({ from: a.from, to: b.to, gain_db: clampDb((a.gain_db ?? 0) + (b.gain_db ?? 0)) });
    }
  return out;
}

/** 線 from → to の間に入れる */
export function splitLink(links: FxLink[], from: string, to: string, id: string): FxLink[] {
  const i = links.findIndex((l) => l.from === from && l.to === to);
  if (i < 0) return links;
  const out = links.filter((_, k) => k !== i);
  out.push({ from, to: id, gain_db: links[i].gain_db }, { from: id, to });
  return out;
}

const clampDb = (v: number) => Math.max(-120, Math.min(24, v));

/** ただの 1 本の直列(分岐・合流・線の音量なし)なら、その順番を返す */
export function serialOrder(links: FxLink[]): string[] | null {
  const order: string[] = [];
  let cur = IN;
  const used = new Set<string>();
  for (;;) {
    const outs = links.filter((l) => l.from === cur);
    if (outs.length !== 1 || (outs[0].gain_db ?? 0) !== 0) return null;
    const next = outs[0].to;
    if (links.filter((l) => l.to === next).length !== 1) return null;
    used.add(linkKey(outs[0]));
    if (next === OUT) break;
    order.push(next);
    cur = next;
  }
  // つながっていないもの同士の線などがあれば、ただの直列ではない
  return used.size === links.length ? order : null;
}

/** 並びの順の直列の線 */
export function serialLinks(ids: string[]): FxLink[] {
  const s = [IN, ...ids, OUT];
  return s.slice(0, -1).map((from, i) => ({ from, to: s[i + 1] }));
}
