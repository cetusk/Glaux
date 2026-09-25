// 音源の名前・アイコン・説明(トラックの見出し・インスペクター・音源ピッカーで共通)。
import type { IconName } from "./icons";
import type { Track } from "./types";

export const BUILTIN_INSTRUMENTS: { name: string; icon: IconName; desc: string }[] = [
  { name: "subtractive", icon: "audio-waveform", desc: "シンセ全般(リード・ベース・パッド)" },
  { name: "drum", icon: "drum", desc: "ドラムシンセ(GM 配置、キット表示)" },
  { name: "pluck", icon: "guitar", desc: "撥弦(ギター・ベース・ハープ)" },
  { name: "fm", icon: "bell", desc: "FM(エレピ・ベル・マレット・FM ベース)" },
  { name: "wavetable", icon: "waves", desc: "ウェーブテーブル(うねるベース・変化するパッド・母音)" },
];

type Device = Track["device"];

/** 音源の種類のアイコン */
export function deviceIcon(device: Device): IconName {
  if (!device) return "audio-waveform";
  if (device.type === "clap") return "plug";
  if (device.type === "sf2") return "library";
  if (device.type === "sampler") return "file-audio";
  return BUILTIN_INSTRUMENTS.find((b) => b.name === device.name)?.icon ?? "audio-waveform";
}

/** 音源の表示名。CLAP はプラグイン名(一覧があれば)、SoundFont はファイル名とプリセット番号 */
export function deviceName(device: Device, clapNames?: Map<string, string>): string {
  if (!device) return "subtractive(未設定)";
  if (device.type === "clap") {
    const id = device.plugin_id ?? "";
    return clapNames?.get(id) ?? id.split(".").pop() ?? "CLAP";
  }
  if (device.type === "sf2") {
    const d = device as { soundfont?: string; bank?: number; preset?: number };
    return `${(d.soundfont ?? "SoundFont").replace(/\.sf2$/i, "")} ${d.bank ?? 0}:${d.preset ?? 0}`;
  }
  if (device.type === "sampler") return "sampler";
  return device.name ?? "subtractive";
}

/** 音源の種類の説明(カードの 2 行目) */
export function deviceKind(device: Device): string {
  if (!device) return "内蔵シンセ(既定)";
  if (device.type === "clap") return "CLAP プラグイン";
  if (device.type === "sf2") return "SoundFont";
  if (device.type === "sampler") return "サンプル(WAV)";
  return "内蔵";
}
