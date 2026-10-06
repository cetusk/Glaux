// エフェクトの名前・色・アイコン(ミキサーの列・ノード表示・インスペクターで共通)。
import type { IconName } from "./icons";
import type { EffectView, FxLink, ProjectEffect } from "./types";
import { tr } from "./i18n.svelte";

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
  convolution: "#8f6fd6",
  resonance: "#c9a86a",
  virtual_bass: "#b86a4c",
  clipper: "#e0913a",
  bitcrush: "#c47b3a",
  tremolo: "#4fb0a0",
  phaser: "#5aa0d8",
  flanger: "#6f8fe0",
  trance_gate: "#d6a33a",
  auto_filter: "#7a9f4f",
  volume_shaper: "#c9b04a",
  eq8: "#5f9be6",
  saturation: "#d9822b",
  deesser: "#8fa6c9",
  gate: "#8c96a3",
  pitch_shift: "#b48be0",
  harmonizer: "#c495d8",
  pitch_correct: "#9b7fe0",
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
  convolution: "畳み込みリバーブ",
  resonance: "共鳴抑制",
  virtual_bass: "仮想低音",
  clipper: "クリッパー",
  bitcrush: "ビットクラッシュ",
  tremolo: "トレモロ",
  phaser: "フェイザー",
  flanger: "フランジャー",
  trance_gate: "トランスゲート",
  auto_filter: "動くフィルター",
  volume_shaper: "音量シェイパー",
  eq8: "8 バンド EQ",
  saturation: "サチュレーション",
  deesser: "ディエッサー",
  gate: "ゲート",
  pitch_shift: "ピッチシフト",
  harmonizer: "ハーモナイザー",
  pitch_correct: "ピッチ補正",
  sidechain: "サイドチェイン",
};

/** 種類の英語名(表示用。FX_KIND_JA と同じキー) */
export const FX_KIND_EN: Record<string, string> = {
  eq: "EQ",
  compressor: "Compressor",
  distortion: "Distortion",
  amp: "Amp",
  reverb: "Reverb",
  delay: "Delay",
  chorus: "Chorus",
  tape: "Tape",
  multiband: "Multiband",
  transient: "Transient",
  limiter: "Limiter",
  width: "Width",
  dynamic_eq: "Dynamic EQ",
  convolution: "Convolution reverb",
  resonance: "Resonance suppressor",
  virtual_bass: "Virtual bass",
  clipper: "Clipper",
  bitcrush: "Bitcrush",
  tremolo: "Tremolo",
  phaser: "Phaser",
  flanger: "Flanger",
  trance_gate: "Trance gate",
  auto_filter: "Auto filter",
  volume_shaper: "Volume shaper",
  eq8: "8-band EQ",
  saturation: "Saturation",
  deesser: "De-esser",
  gate: "Gate",
  pitch_shift: "Pitch shift",
  harmonizer: "Harmonizer",
  pitch_correct: "Pitch correct",
  sidechain: "Sidechain",
};

/** 種類の説明の英語(追加メニューの小さな説明。日本語は裏側の説明〈AI 向けを兼ねる〉をそのまま出す) */
export const FX_DESC_EN: Record<string, string> = {
  eq: "EQ (high-pass, low shelf, mid peak, high shelf, low-pass). Adjusts tonal balance.",
  compressor: "Compressor. Evens out level and adds loudness and density.",
  distortion: "Distortion / saturation pedal. Gritty texture, thicker drums, or a boost before an amp.",
  amp: "Guitar amp simulator (multi-stage clipping, tone, presence, cabinet).",
  reverb: "Reverb. Adds depth and space.",
  delay: "Delay. Tempo-synced echoes for depth and tails; ping-pong spreads left and right.",
  chorus: "Chorus. Layers delayed, modulated copies for thickness and stereo width.",
  tape: "Tape / lo-fi. Wow and flutter, tape saturation, dulled highs, hiss and bit reduction for a vintage feel.",
  multiband: "Multiband compressor. Controls dynamics per frequency band.",
  transient: "Transient shaper. Boosts or cuts attack and sustain separately, regardless of level.",
  limiter: "Limiter. Raises loudness while keeping peaks under the ceiling.",
  width: "Stereo width (M/S). Narrows or widens the image, centers the lows; stays mono-safe.",
  dynamic_eq: "Dynamic EQ. Cuts or boosts a band only when it gets loud.",
  convolution: "Convolution reverb. Reverb from a recorded impulse response.",
  resonance: "Resonance suppressor. Automatically dips narrow, harsh peaks only while they ring.",
  virtual_bass: "Virtual bass. Adds harmonics so bass and kick are heard on phones and laptops.",
  clipper: "Clipper. Shaves peaks for loudness and punch (soft / hard / fold).",
  bitcrush: "Bitcrusher. Lowers bit depth and sample rate for a gritty lo-fi / game-console sound.",
  tremolo: "Tremolo / auto-pan. Tempo-synced volume wobble (stereo 1 pans left and right).",
  phaser: "Phaser. Swirling, swooshing movement.",
  flanger: "Flanger. Metallic, jet-like sweeps.",
  trance_gate: "Trance gate. Chops the sound in a 16th-note pattern synced to tempo.",
  auto_filter: "Auto filter. Cutoff moved by a tempo-synced LFO or the input level (risers, auto-wah).",
  volume_shaper: "Volume shaper. Ducks and recovers each cycle (kick-style pumping without a sidechain).",
  eq8: "8-band parametric EQ with per-band types, dynamics and stereo / mid / side processing.",
  saturation: "Saturation (tape / tube / transistor / soft clip). Adds harmonics, warmth and density.",
  deesser: "De-esser. Turns down vocal sibilance only while it occurs.",
  gate: "Gate. Closes below a threshold; with a source track, opens only while that track plays.",
  pitch_shift: "Pitch shifter. Moves pitch without changing length (±24 semitones + cents).",
  harmonizer: "Harmonizer. Layers 1–2 voices at a fixed interval (3rds, 5ths…) for harmonies.",
  pitch_correct: "Pitch correction. Pulls a monophonic voice toward the nearest note of the key and scale.",
  sidechain: "Sidechain compressor. Ducks this track when another track (usually the kick) hits.",
};

/** 種類の説明(日本語のときは `ja` のまま。英語の対訳が無ければ空) */
export function fxKindDesc(kind: string, ja: string): string {
  return tr(ja, FX_DESC_EN[kind] ?? "");
}

/** 種類の表示名(今の表示の言語。表に無ければ undefined)。テンプレート・$derived の中で呼べば言語の切り替えで描き直される */
export function fxKindName(kind: string): string | undefined {
  return tr(FX_KIND_JA[kind], FX_KIND_EN[kind]) ?? FX_KIND_JA[kind];
}

/** 選択肢が空の「検出のトラック」(サイドチェイン・ダイナミック EQ・ゲートの source)は、曲のトラックから選ばせる */
export function trackChoices(
  p: { name: string; range: { kind: string; choices?: readonly string[] } },
  tracks: { id: string; name: string }[],
): { value: string; label: string }[] | null {
  if (p.name !== "source" || p.range.kind !== "enum" || (p.range.choices?.length ?? 0) > 0) return null;
  return [{ value: "", label: tr("なし(自分の音)", "None (own signal)") }, ...tracks.map((t) => ({ value: t.id, label: t.name }))];
}

/** 選択肢の「ファイルから読み込む…」 */
export const IR_FROM_FILE = "__file__";

/** 選択肢が空の「響き(IR)」(畳み込みリバーブの ir)は、プロジェクトの音声素材から選ぶか、ファイルから読み込む */
export function irChoices(
  p: { name: string; range: { kind: string; choices?: readonly string[] } },
  assets: Record<string, unknown>,
): { value: string; label: string }[] | null {
  if (p.name !== "ir" || p.range.kind !== "enum" || (p.range.choices?.length ?? 0) > 0) return null;
  const items = Object.entries(assets).map(([id, a]) => {
    const path = (a as { path?: string })?.path ?? id;
    return { value: id, label: path.split(/[\\/]/).pop() ?? path };
  });
  return [{ value: "", label: tr("なし(素通し)", "None (bypass)") }, ...items, { value: IR_FROM_FILE, label: tr("ファイルから読み込む…", "Load from file…") }];
}

/** 選択肢が空の「素材」(グラニュラーの sample)は、プロジェクトの音声素材から選ぶ */
export function sampleChoices(
  p: { name: string; range: { kind: string; choices?: readonly string[] } },
  assets: Record<string, unknown>,
): { value: string; label: string }[] | null {
  if (p.name !== "sample" || p.range.kind !== "enum" || (p.range.choices?.length ?? 0) > 0) return null;
  const items = Object.entries(assets).map(([id, a]) => {
    const path = (a as { path?: string })?.path ?? id;
    return { value: id, label: path.split(/[\\/]/).pop() ?? path };
  });
  return [{ value: "", label: tr("なし(無音)", "None (silent)") }, ...items];
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
  if (a === b) return tr("自分自身にはつなげない", "Can't connect to itself");
  if (a === OUT || b === IN) return tr("向きが逆", "Wrong direction");
  if (links.some((l) => l.from === a && l.to === b)) return tr("もうつながっている", "Already connected");
  if (reach(links, b, true).has(a)) return tr("輪になるのでつなげない", "Would create a loop");
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
