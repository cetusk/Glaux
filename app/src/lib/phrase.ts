// 音作り用の試聴フレーズ(4 小節)。
// 音の判断に必要な要素を一巡する: ロングトーン(立ち上がり・持続・リリース)
// → 8 分の刻み(アタック感)→ 分散和音(音の重なり)→ オクターブ上(音域差)。
// SoundDesignPanel の「試聴フレーズを挿入」と SoundLab テンプレートが共用する。

import { newNoteId } from "./ids";
import type { Note } from "./types";

export const PHRASE_NAME = "試聴フレーズ";
export const PHRASE_LEN = 3840 * 4;

export function phraseNotes(): Note[] {
  const notes: Note[] = [];
  notes.push({ id: newNoteId(), pos: 0, dur: 3840, pitch: 48, vel: 100 }); // ロングトーン
  for (let i = 0; i < 8; i++) {
    notes.push({ id: newNoteId(), pos: 3840 + i * 480, dur: 240, pitch: 48, vel: 100 }); // 刻み
  }
  for (const [i, pitch] of [48, 52, 55, 60].entries()) {
    notes.push({ id: newNoteId(), pos: 7680 + i * 960, dur: 720, pitch, vel: 100 }); // 分散和音
  }
  notes.push({ id: newNoteId(), pos: 11520, dur: 3840, pitch: 60, vel: 100 }); // オクターブ上
  return notes;
}
