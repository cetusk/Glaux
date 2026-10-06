//! 人に見せる文の言語(日本語 / English)。アプリの設定の「言語」に合わせて、アプリが [`set_english`] で切り替える。
//! AI に向けた文(道具の説明・定石・AI への指示)は日本語のまま。人が画面で読む文(履歴の名前・計画と実際のずれの説明・
//! 画面に出るエラー)だけを [`tr!`](crate::tr) / [`t`] で書き分ける。既定は日本語(stdio の MCP サーバー・テスト)
use std::sync::atomic::{AtomicBool, Ordering};

static ENGLISH: AtomicBool = AtomicBool::new(false);

/// 人に見せる文を英語にするか
pub fn set_english(on: bool) {
    ENGLISH.store(on, Ordering::Relaxed);
}

/// 今、人に見せる文が英語か
pub fn is_en() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

/// 日本語と英語の対訳から、今の言語の方を返す(固定の文)
pub fn t(ja: &'static str, en: &'static str) -> &'static str {
    if is_en() {
        en
    } else {
        ja
    }
}

/// 日本語と英語の書式から、今の言語の方で `format!` する: `tr!("{n} 小節", "{n} bars")`
#[macro_export]
macro_rules! tr {
    ($ja:literal, $en:literal $(, $arg:expr)* $(,)?) => {
        if $crate::i18n::is_en() {
            format!($en $(, $arg)*)
        } else {
            format!($ja $(, $arg)*)
        }
    };
}

#[cfg(test)]
mod tests {
    #[test]
    fn picks_the_language() {
        let n = 3;
        assert_eq!(crate::tr!("{n} 小節", "{n} bars"), "3 小節");
        assert_eq!(super::t("区間", "Section"), "区間");
    }
}
