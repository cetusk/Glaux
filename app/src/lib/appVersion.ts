/// アプリの版の表記。ビルド番号が分かれば 0.x.y.z(z = ビルド番号)、分からなければ 0.x.y
export const APP_VERSION: string = __APP_BUILD__ ? `${__APP_VERSION__}.${__APP_BUILD__}` : __APP_VERSION__;

/// 版の説明(ビルド番号とコミット)
export const APP_VERSION_DETAIL: string = [
  `Glaux ${__APP_VERSION__}`,
  __APP_BUILD__ ? `ビルド ${__APP_BUILD__}` : "",
  __APP_COMMIT__ ? `コミット ${__APP_COMMIT__}` : "",
]
  .filter(Boolean)
  .join("・");
