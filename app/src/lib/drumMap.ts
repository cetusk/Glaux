// ドラムキットのマッピング(glaux-dsp の drum.rs の GM 配置と対応)。
// 将来: キットを差し替え可能にする際は、この定数を dsp 側のレジストリ
// (MCP list_params 経由)から供給する形に移行する。

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

export const DRUM_PIECES: DrumPiece[] = [
  { pitch: 49, name: "クラッシュ", short: "CR", cx: 70, cy: 46, r: 34, kind: "cymbal" },
  { pitch: 42, name: "ハイハット(閉)", short: "HH", cx: 36, cy: 122, r: 22, kind: "cymbal" },
  { pitch: 46, name: "ハイハット(開)", short: "OH", cx: 86, cy: 145, r: 19, kind: "cymbal" },
  { pitch: 48, name: "ハイタム", short: "T1", cx: 163, cy: 55, r: 24, kind: "drum" },
  { pitch: 45, name: "ミッドタム", short: "T2", cx: 224, cy: 55, r: 27, kind: "drum" },
  { pitch: 41, name: "フロアタム", short: "FT", cx: 278, cy: 122, r: 32, kind: "drum" },
  { pitch: 51, name: "ライド", short: "RD", cx: 330, cy: 50, r: 38, kind: "cymbal" },
  { pitch: 38, name: "スネア", short: "SD", cx: 138, cy: 122, r: 28, kind: "drum" },
  { pitch: 36, name: "キック", short: "BD", cx: 197, cy: 128, r: 38, kind: "drum" },
  { pitch: 39, name: "クラップ", short: "CP", cx: 352, cy: 135, r: 20, kind: "pad" },
];

const byPitch = new Map(DRUM_PIECES.map((p) => [p.pitch, p]));

export function drumName(pitch: number): DrumPiece | undefined {
  return byPitch.get(pitch);
}
