// 畳み込みリバーブの響き(IR)をファイルから読み込む(ノード表示・インスペクター共通)
import { open as pickFile } from "@tauri-apps/plugin-dialog";
import * as api from "./api";
import { showError, showToast } from "./toast.svelte";

/** `path` は `fx/<fx_id>/ir`。trackId が null ならマスター */
export async function loadIrFromFile(trackId: string | null, path: string) {
  const fxId = path.split("/")[1];
  if (!fxId) return;
  const file = await pickFile({
    title: "響き(インパルス応答)の音声を読み込む",
    filters: [{ name: "音声(WAV / MP3 / FLAC / OGG / M4A)", extensions: ["wav", "mp3", "flac", "ogg", "m4a", "aac"] }],
  });
  if (typeof file !== "string") return;
  try {
    await api.importIr(trackId, fxId, file);
    showToast("ok", "響きを読み込みました");
  } catch (e) {
    showError("響きを読み込めませんでした", e);
  }
}
