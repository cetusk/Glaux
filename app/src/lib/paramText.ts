// 音源・エフェクトのつまみの名前と説明の英語(つまみの定義〈glaux-dsp の ParamSpec〉は日本語の表示名と AI 向けの説明を持つ)。
// 英語のときだけ、ここの対訳を出す。kind は音源・エフェクトの種類(subtractive / reverb など。分からなければ null)、
// name はつまみの名前(cutoff など。定義の name)。表に無ければ、名前を読みやすくしたもの(filter_env → Filter env)
import { isEn } from "./i18n.svelte";

/** つまみの表示名の英語(`kind:name` か `name` で引く) */
const LABEL_EN: Record<string, string> = {};

/** つまみの説明の英語(`kind:name` か `name` で引く) */
const DESC_EN: Record<string, string> = {};

/** 名前を読みやすく: filter_env → Filter env、gain_db → Gain (dB) */
function humanize(name: string): string {
  const unit = name.endsWith("_db") ? " (dB)" : name.endsWith("_ms") ? " (ms)" : name.endsWith("_hz") ? " (Hz)" : "";
  const base = name.replace(/_(db|ms|hz)$/, "").replace(/_/g, " ");
  return base.charAt(0).toUpperCase() + base.slice(1) + unit;
}

/** つまみの表示名(日本語のときは `ja` のまま) */
export function paramLabel(kind: string | null | undefined, name: string, ja: string): string {
  if (!isEn()) return ja;
  return (kind && LABEL_EN[`${kind}:${name}`]) || LABEL_EN[name] || humanize(name);
}

/** つまみの説明(日本語のときは `ja` のまま。英語の対訳が無ければ空) */
export function paramDesc(kind: string | null | undefined, name: string, ja: string): string {
  if (!isEn()) return ja;
  return (kind && DESC_EN[`${kind}:${name}`]) || DESC_EN[name] || "";
}
