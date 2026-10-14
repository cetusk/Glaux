// 参考曲の聴き比べと遅い再生の状態(画面全体で 1 つ)。
//
// 参考曲は「参考」の印を付けた音声トラック(track.reference)。曲の音には混ぜず、上の帯の切り替えで
// 「自分の曲 / 参考曲 / 重ねる」を聴き分ける。切り替えると音量をそろえる倍率を測り直す(大きい方が良く聞こえる
// 錯覚を避けるため。曲が変わっていれば測り直す)。遅い再生は音程を変えずに曲も参考曲も同じ速さで遅くする。
import { open as pickFile } from "@tauri-apps/plugin-dialog";
import * as api from "./api";
import { tr } from "./i18n.svelte";
import { showError, showToast } from "./toast.svelte";
import type { Project, Track } from "./types";

export const referenceStore = $state<{
  mode: api.ReferenceMode;
  /** 再生の速さ(1 / 0.75 / 0.5) */
  speed: number;
  /** 音量をそろえた結果(測っていなければ null) */
  match: Awaited<ReturnType<typeof api.referenceMatchLoudness>> | null;
  /** 音量を測っている途中 */
  matching: boolean;
  /** 参考曲の骨組みの要約(クリップ ID → 文) */
  summaries: Record<string, string>;
}>({ mode: "off", speed: 1, match: null, matching: false, summaries: {} });

/** 音量を測ったときの曲の版(変わっていれば測り直す) */
let matchedVersion = -1;
/** 骨組みを解析中のクリップ */
const analyzing = new Set<string>();

/** 曲の参考曲のトラック(最初の 1 本) */
export function referenceTrack(project: Project | null | undefined): Track | undefined {
  return project?.tracks.find((t) => t.reference);
}

/** 参考曲のトラックの最初の音声クリップ */
export function referenceClipId(project: Project | null | undefined): string | undefined {
  const t = referenceTrack(project);
  const c = t?.clips.find((c) => c.kind === "audio");
  return c?.id;
}

/** 参考曲を読み込む(ファイルを選んで、いちばん上に参考曲のトラックを作る) */
export async function importReferenceFile() {
  const file = await pickFile({
    title: tr("参考曲を読み込む", "Load a reference song"),
    filters: [{ name: tr("音声(WAV / MP3 / FLAC / OGG / M4A)", "Audio (WAV / MP3 / FLAC / OGG / M4A)"), extensions: ["wav", "mp3", "flac", "ogg", "m4a", "aac"] }],
  });
  if (typeof file !== "string") return;
  try {
    const r = await api.importReference(file);
    matchedVersion = -1;
    showToast(
      "ok",
      tr(
        "参考曲を読み込みました。上の帯の「参考曲」で聴き比べられます(曲の音・書き出しには入りません)",
        "Reference loaded. Compare with “Reference” on the top bar (it isn't mixed into the song or exports)",
      ),
    );
    void analyzeReference(r.clip_id);
  } catch (e) {
    showError(tr("参考曲を読み込めませんでした", "Couldn't load the reference"), e);
  }
}

/** 参考曲の骨組み(テンポ・拍子・調)を解析して要約を覚える(1 クリップ 1 回) */
export async function analyzeReference(clipId: string) {
  if (referenceStore.summaries[clipId] || analyzing.has(clipId)) return;
  analyzing.add(clipId);
  try {
    const sk = await api.referenceSkeleton(clipId);
    referenceStore.summaries[clipId] = sk.summary;
  } catch {
    // 短すぎる・拍の無い音などは要約なし
  } finally {
    analyzing.delete(clipId);
  }
}

/** 聴き方を変える。聴き比べ・重ねるときは、曲が変わっていれば音量をそろえ直す */
export async function setReferenceMode(mode: api.ReferenceMode, projectVersion: number, range?: [number, number] | null) {
  referenceStore.mode = mode;
  await api.referenceSetMode(mode).catch(() => {});
  if (mode === "off" || referenceStore.matching || projectVersion === matchedVersion) return;
  referenceStore.matching = true;
  try {
    referenceStore.match = await api.referenceMatchLoudness(range?.[0], range?.[1]);
    matchedVersion = projectVersion;
  } catch (e) {
    showError(tr("参考曲と音量をそろえられませんでした", "Couldn't match loudness with the reference"), e);
  } finally {
    referenceStore.matching = false;
  }
}

/** 曲を開き直したとき・参考曲が無くなったとき: ふつうの聴き方に戻す */
export function resetReference() {
  matchedVersion = -1;
  referenceStore.match = null;
  if (referenceStore.mode !== "off") {
    referenceStore.mode = "off";
    void api.referenceSetMode("off").catch(() => {});
  }
}

/** 遅い再生の段(押すたびに 100% → 75% → 50% → 100%) */
export const SPEEDS = [1, 0.75, 0.5];

export async function setSpeed(speed: number) {
  referenceStore.speed = speed;
  await api.transportSetSpeed(speed).catch(() => {});
}
