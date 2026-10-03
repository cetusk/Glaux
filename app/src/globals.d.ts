/// ビルド時に vite.config.ts の define で埋める
declare const __APP_VERSION__: string;
/// ビルド番号(Git のコミットの通し番号。分からなければ空)と短いコミット ID
declare const __APP_BUILD__: string;
declare const __APP_COMMIT__: string;
