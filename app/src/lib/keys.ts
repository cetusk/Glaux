// キー操作を、フォーカスのある入力欄に譲るべきかの判定(App・Timeline・PianoRoll で共通)。
//
// 以前は INPUT なら何でも譲っていたので、音量スライダー(type=range)を触った直後は
// Ctrl+Z・Space・Delete が効かなかった。文字を打つ欄だけは全部譲り、スライダー等は
// 値を動かすキー(矢印など)だけ譲る。

const SLIDER_KEYS = new Set([
  "ArrowLeft",
  "ArrowRight",
  "ArrowUp",
  "ArrowDown",
  "Home",
  "End",
  "PageUp",
  "PageDown",
]);

const NON_TEXT_INPUTS = new Set(["range", "checkbox", "radio", "button", "submit", "reset", "color", "file"]);

/** このキー操作をアプリのショートカットにせず、フォーカスのある要素に任せるか */
export function shouldYieldKey(e: KeyboardEvent): boolean {
  const el = e.target as HTMLElement | null;
  if (!el || !el.tagName) return false;
  if (el.isContentEditable || el.tagName === "TEXTAREA") return true;
  if (el.tagName === "SELECT") {
    // 選択肢は頭文字や矢印で選ぶので、修飾キーなしは譲る(Ctrl+Z 等はアプリへ)
    return !(e.ctrlKey || e.metaKey || e.altKey);
  }
  if (el.tagName === "INPUT") {
    const type = (el as HTMLInputElement).type;
    if (!NON_TEXT_INPUTS.has(type)) return true; // text / number / search など
    return type === "range" && SLIDER_KEYS.has(e.code);
  }
  return false;
}
