import { tr } from "./i18n.svelte";

/// アプリの版の表記。ビルド番号が分かれば 0.x.y.z(z = ビルド番号)、分からなければ 0.x.y
export const APP_VERSION: string = __APP_BUILD__ ? `${__APP_VERSION__}.${__APP_BUILD__}` : __APP_VERSION__;

/// 版の説明(ビルド番号とコミット)。今の表示の言語で作る(テンプレートの中で呼べば言語の切り替えに追従する)
export function appVersionDetail(): string {
  return [
    `Glaux ${__APP_VERSION__}`,
    __APP_BUILD__ ? tr(`ビルド ${__APP_BUILD__}`, `build ${__APP_BUILD__}`) : "",
    __APP_COMMIT__ ? tr(`コミット ${__APP_COMMIT__}`, `commit ${__APP_COMMIT__}`) : "",
  ]
    .filter(Boolean)
    .join(tr("・", " · "));
}

/// 版の説明(起動時の言語で一度だけ作る。言語の切り替えに追従させるには appVersionDetail() を使う)
export const APP_VERSION_DETAIL: string = appVersionDetail();
