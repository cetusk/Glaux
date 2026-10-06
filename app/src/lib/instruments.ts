// 音源の名前・アイコン・説明(トラックの見出し・インスペクター・音源ピッカーで共通)。
import { tr } from "./i18n.svelte";
import type { IconName } from "./icons";
import type { Track } from "./types";

/** 説明は表示の言語で返す(読むたびに選ぶ) */
function builtin(name: string, icon: IconName, ja: string, en: string): { name: string; icon: IconName; readonly desc: string } {
  return {
    name,
    icon,
    get desc() {
      return tr(ja, en);
    },
  };
}

export const BUILTIN_INSTRUMENTS: { name: string; icon: IconName; readonly desc: string }[] = [
  builtin("subtractive", "audio-waveform", "シンセ全般(リード・ベース・パッド)", "General synth (leads, basses, pads)"),
  builtin("drum", "drum", "ドラムシンセ(GM 配置、キット表示)", "Drum synth (GM layout, kit view)"),
  builtin("pluck", "guitar", "撥弦(ギター・ベース・ハープ)", "Plucked strings (guitar, bass, harp)"),
  builtin("fm", "bell", "FM(エレピ・ベル・マレット・FM ベース)", "FM (e-piano, bells, mallets, FM bass)"),
  builtin("wavetable", "waves", "ウェーブテーブル(うねるベース・変化するパッド・母音)", "Wavetable (wobbly basses, evolving pads, vowels)"),
  builtin("fm4", "bell", "4 オペレーター FM(DX のエレピ・ベル・ブラス・オルガン)", "4-operator FM (DX e-piano, bells, brass, organ)"),
  builtin("additive", "audio-waveform", "加算合成(澄んだパッド・オルガン・声のような音)", "Additive (clear pads, organs, voice-like tones)"),
  builtin("granular", "waves", "グラニュラー(取り込んだ音声から粒の雲・パッド。素材は音作りで選ぶ)", "Granular (grain clouds and pads from imported audio; pick the source in the inspector)"),
];

type Device = Track["device"];

/// 新しく選んだ内蔵の音源の初期値。減算・ウェーブテーブルは「生きた音」寄り(揺らぎ 0.2・広がり 0.5)、
/// 加算合成は部分音の揺らぎ 0.15 で始める
/// (既定値は従来と同じ音のまま。既存の曲は変わらない。MCP の add_track も同じ値を入れる)
export const LIVELY_PARAMS: Record<string, Record<string, number>> = {
  subtractive: { analog: 0.2, spread: 0.5 },
  wavetable: { analog: 0.2, spread: 0.5 },
  additive: { wobble: 0.15 },
};

/** 内蔵の音源の device(新しく選んだとき) */
export function builtinDevice(name: string) {
  const params = LIVELY_PARAMS[name];
  return params ? { type: "builtin", name, params: { ...params } } : { type: "builtin", name };
}

/** 音源の種類のアイコン */
export function deviceIcon(device: Device): IconName {
  if (!device) return "audio-waveform";
  if (device.type === "clap") return "plug";
  if (device.type === "sf2" || device.type === "sfz") return "library";
  if (device.type === "sampler") return "file-audio";
  return BUILTIN_INSTRUMENTS.find((b) => b.name === device.name)?.icon ?? "audio-waveform";
}

/** 音源の表示名。CLAP はプラグイン名(一覧があれば)、SoundFont はファイル名とプリセット番号 */
export function deviceName(device: Device, clapNames?: Map<string, string>): string {
  if (!device) return tr("subtractive(未設定)", "subtractive (not set)");
  if (device.type === "clap") {
    const id = device.plugin_id ?? "";
    return clapNames?.get(id) ?? id.split(".").pop() ?? "CLAP";
  }
  if (device.type === "sf2") {
    const d = device as { soundfont?: string; bank?: number; preset?: number };
    return `${(d.soundfont ?? "SoundFont").replace(/\.sf2$/i, "")} ${d.bank ?? 0}:${d.preset ?? 0}`;
  }
  if (device.type === "sfz") {
    const file = (device.instrument ?? "SFZ").split(/[\\/]/).pop() ?? "SFZ";
    return file.replace(/\.sfz$/i, "");
  }
  if (device.type === "sampler") return "sampler";
  return device.name ?? "subtractive";
}

/** 音源の種類の説明(カードの 2 行目) */
export function deviceKind(device: Device): string {
  if (!device) return tr("内蔵シンセ(既定)", "Built-in synth (default)");
  if (device.type === "clap") return tr("CLAP プラグイン", "CLAP plugin");
  if (device.type === "sf2") return "SoundFont";
  if (device.type === "sfz") return "SFZ";
  if (device.type === "sampler") return tr("サンプル(WAV)", "Sample (WAV)");
  return tr("内蔵", "Built-in");
}
