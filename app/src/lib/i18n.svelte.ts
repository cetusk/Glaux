// 画面の言語(日本語 / English)。画面の文字は `tr("日本語", "English")` で書き、設定の言語の方を出す。
// 設定の言語は、AI の返答の言語も兼ねる(settings.lang)。`tr` は設定を読むので、テンプレート・$derived の中で呼べば
// 言語を切り替えたときに描き直される。モジュールの読み込み時に一度だけ作る表は、関数にするか両方を持って使う所で選ぶ
import { settings } from "./settings.svelte";

/** 今の表示の言語が英語か */
export function isEn(): boolean {
  return settings.lang === "en";
}

/** 日本語と英語の対訳から、今の表示の言語の方を返す */
export function tr(ja: string, en: string): string {
  return settings.lang === "en" ? en : ja;
}
