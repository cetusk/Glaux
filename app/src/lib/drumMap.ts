// ドラムキットのマッピング(glaux-dsp の drum.rs の GM 配置と対応)。
// 将来: キットを差し替え可能にする際は、この定数を dsp 側のレジストリ
// (MCP list_params 経由)から供給する形に移行する。
import { tr } from "./i18n.svelte";

export interface DrumPiece {
  pitch: number;
  name: string;
  short: string;
  /** 真上から見た図での配置(viewBox 400x150) */
  cx: number;
  cy: number;
  r: number;
  kind: "drum" | "cymbal" | "pad";
}

/** 名前は表示の言語で返す(読むたびに選ぶ) */
function piece(ja: string, en: string, p: Omit<DrumPiece, "name">): DrumPiece {
  return {
    ...p,
    get name() {
      return tr(ja, en);
    },
  };
}

export const DRUM_PIECES: DrumPiece[] = [
  piece("クラッシュ", "Crash", { pitch: 49, short: "CR", cx: 70, cy: 46, r: 34, kind: "cymbal" }),
  piece("ハイハット(閉)", "Hi-hat (closed)", { pitch: 42, short: "HH", cx: 36, cy: 122, r: 22, kind: "cymbal" }),
  piece("ハイハット(開)", "Hi-hat (open)", { pitch: 46, short: "OH", cx: 86, cy: 145, r: 19, kind: "cymbal" }),
  piece("ハイタム", "High tom", { pitch: 48, short: "T1", cx: 163, cy: 55, r: 24, kind: "drum" }),
  piece("ミッドタム", "Mid tom", { pitch: 45, short: "T2", cx: 224, cy: 55, r: 27, kind: "drum" }),
  piece("フロアタム", "Floor tom", { pitch: 41, short: "FT", cx: 278, cy: 122, r: 32, kind: "drum" }),
  piece("ライド", "Ride", { pitch: 51, short: "RD", cx: 330, cy: 50, r: 38, kind: "cymbal" }),
  piece("スネア", "Snare", { pitch: 38, short: "SD", cx: 138, cy: 122, r: 28, kind: "drum" }),
  piece("キック", "Kick", { pitch: 36, short: "BD", cx: 197, cy: 128, r: 38, kind: "drum" }),
  piece("クラップ", "Clap", { pitch: 39, short: "CP", cx: 352, cy: 135, r: 20, kind: "pad" }),
];

const byPitch = new Map(DRUM_PIECES.map((p) => [p.pitch, p]));

export function drumName(pitch: number): DrumPiece | undefined {
  return byPitch.get(pitch);
}
