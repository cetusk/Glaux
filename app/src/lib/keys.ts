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

/** 修飾キーなしの Space(再生 / 一時停止) */
const plainSpace = (e: KeyboardEvent) => e.code === "Space" && !(e.ctrlKey || e.metaKey || e.altKey || e.shiftKey);

/** このキー操作をアプリのショートカットにせず、フォーカスのある要素に任せるか */
export function shouldYieldKey(e: KeyboardEvent): boolean {
  const el = e.target as HTMLElement | null;
  if (!el || !el.tagName) return false;
  if (e.isComposing) return true; // 日本語の変換中のキーはすべて入力欄へ
  const isText =
    el.isContentEditable ||
    el.tagName === "TEXTAREA" ||
    (el.tagName === "INPUT" && !NON_TEXT_INPUTS.has((el as HTMLInputElement).type));
  if (isText) {
    // 空の入力欄の Space は再生に回す(チャット欄などに入力先が残ったまま Space を押すと、見えない空白が入るだけで
    // 再生されなかった。先頭の空白は使わないので、文字があるときだけ欄に任せる)。
    // 日本語入力がオンだと Space で全角の空白(U+3000)が入り、以後「空でない」になって効かなくなっていたので、
    // 空白だけの欄も空とみなす(trim は U+3000 も取る)
    const value = el.isContentEditable ? (el.textContent ?? "") : (el as HTMLInputElement | HTMLTextAreaElement).value;
    return !(plainSpace(e) && value.trim() === "");
  }
  if (el.tagName === "SELECT") {
    // 選択肢は頭文字や矢印で選ぶので、修飾キーなしは譲る(Ctrl+Z 等はアプリへ)。
    // Space は再生に回す(スナップなどの選択欄を触った後、入力先が残って Space が効かなかった)
    return !(e.ctrlKey || e.metaKey || e.altKey) && !plainSpace(e);
  }
  if (el.tagName === "INPUT") {
    return (el as HTMLInputElement).type === "range" && SLIDER_KEYS.has(e.code);
  }
  return false;
}

/** 空の入力欄で Space を再生に回したとき、日本語入力が後から入れる全角の空白を消す。
 *  日本語入力がオンの Space(keyCode 229)は preventDefault しても入力を止められないので、続く入力を見て空に戻す */
export function dropImeSpace(e: KeyboardEvent): void {
  const el = e.target as HTMLInputElement | HTMLTextAreaElement | null;
  if (!el || (el.tagName !== "TEXTAREA" && el.tagName !== "INPUT")) return;
  const clear = () => {
    if (el.value !== "" && el.value.trim() === "") {
      el.value = "";
      // bind:value の側にも知らせる
      el.dispatchEvent(new Event("input", { bubbles: true }));
    }
  };
  const onInput = () => {
    el.removeEventListener("input", onInput);
    clear();
  };
  el.addEventListener("input", onInput);
  setTimeout(() => el.removeEventListener("input", onInput), 500);
}
