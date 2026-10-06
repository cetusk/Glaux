// 畳み込みリバーブの響き(IR)をファイルから読み込む(ノード表示・インスペクター共通)
import { open as pickFile } from "@tauri-apps/plugin-dialog";
import * as api from "./api";
import { tr } from "./i18n.svelte";
import { showError, showToast } from "./toast.svelte";

/** `path` は `fx/<fx_id>/ir`。trackId が null ならマスター */
export async function loadIrFromFile(trackId: string | null, path: string) {
  const fxId = path.split("/")[1];
  if (!fxId) return;
  const file = await pickFile({
    title: tr("響き(インパルス応答)の音声を読み込む", "Load an impulse response (IR) audio file"),
    filters: [{ name: tr("音声(WAV / MP3 / FLAC / OGG / M4A)", "Audio (WAV / MP3 / FLAC / OGG / M4A)"), extensions: ["wav", "mp3", "flac", "ogg", "m4a", "aac"] }],
  });
  if (typeof file !== "string") return;
  try {
    await api.importIr(trackId, fxId, file);
    showToast("ok", tr("響きを読み込みました", "Impulse response loaded"));
  } catch (e) {
    showError(tr("響きを読み込めませんでした", "Couldn't load the impulse response"), e);
  }
}
