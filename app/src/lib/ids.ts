// glaux の ID 規約(<prefix>_ + base36 6 桁)に沿った新規 ID 生成。
// コマンドは決定的(ID は作る側が渡す)なので、UI 編集では UI が生成する。

const CHARS = "0123456789abcdefghijklmnopqrstuvwxyz";

function base36(len: number): string {
  let s = "";
  for (let i = 0; i < len; i++) {
    s += CHARS[Math.floor(Math.random() * CHARS.length)];
  }
  return s;
}

export function newNoteId(): string {
  return `nt_${base36(6)}`;
}

export function newClipId(): string {
  return `clp_${base36(6)}`;
}

export function newTrackId(): string {
  return `trk_${base36(6)}`;
}

export function newFxId(): string {
  return `fx_${base36(6)}`;
}
