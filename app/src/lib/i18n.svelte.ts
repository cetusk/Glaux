// 画面の言語(日本語 / English)。画面の文字は `tr("日本語", "English")` で書き、設定の言語の方を出す。
// 設定の言語は、AI の返答の言語も兼ねる(settings.lang)。`tr` は設定を読むので、テンプレート・$derived の中で呼べば
// 言語を切り替えたときに描き直される。モジュールの読み込み時に一度だけ作る表は、関数にするか両方を持って使う所で選ぶ
import { invoke } from "@tauri-apps/api/core";
import { settings } from "./settings.svelte";

/** 裏側(Rust)で作る、人に見せる文(履歴の名前・ずれの説明・エラー)の言語を合わせる。起動時と言語を変えたときに呼ぶ */
export function syncBackendLanguage(): void {
  invoke("set_ui_language", { lang: settings.lang }).catch(() => undefined);
}

/** 今の表示の言語が英語か */
export function isEn(): boolean {
  return settings.lang === "en";
}

/** 日本語と英語の対訳から、今の表示の言語の方を返す */
export function tr(ja: string, en: string): string {
  return settings.lang === "en" ? en : ja;
}

/** 英語の数と名詞(1 なら単数形): plural(3, "edit") → "3 edits" */
export function plural(n: number, word: string, many = `${word}s`): string {
  return `${n} ${n === 1 ? word : many}`;
}
