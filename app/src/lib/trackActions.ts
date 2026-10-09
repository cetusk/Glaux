// タイムラインのトラックとクリップの操作のうち、画面の状態に頼らないもの(すべて Command API 経由、author: human)
import { open as pickFile } from "@tauri-apps/plugin-dialog";
import * as api from "./api";
import { barAtTick, type Bar } from "./barMap";
import type { ClipState } from "./design.svelte";
import { plural, tr } from "./i18n.svelte";
import { newClipId, newFxId, newTrackId } from "./ids";
import { builtinDevice } from "./instruments";
import { showError, showToast } from "./toast.svelte";
import type { Project, Track } from "./types";

export function setTrackVolume(t: Track, v: number) {
  api
    .applyEdit(
      [{ op: "set_track_prop", id: t.id, prop: "volume_db", value: v }],
      tr(`${t.name} の音量を ${v.toFixed(1)} dB に変更`, `Set ${t.name} volume to ${v.toFixed(1)} dB`),
    )
    .catch(() => {});
}

/// ミュートかソロを切り替える。`ts` は対象(選んだトラック全部)で、押したトラック `t` の新しい状態にそろえる
/// ミュートかソロを切り替える。`ts` は対象(選んだトラック全部)で、押したトラック `t` の新しい状態にそろえる。
/// 聴き方なので履歴に積まない(いくつか undo してソロで聴き直してから redo できる)
export function toggleTrackFlag(prop: "mute" | "solo", t: Track, ts: Track[]) {
  const value = !t[prop];
  api.setListen(ts.map((x) => ({ op: "set_track_prop", id: x.id, prop, value }))).catch(() => {});
}

export function renameTrack(track: Track, value: string) {
  const name = value.trim();
  if (!name || name === track.name) return;
  api
    .applyEdit(
      [{ op: "set_track_prop", id: track.id, prop: "name", value: name }],
      tr(`トラック名を「${track.name}」から「${name}」に変更`, `Rename track "${track.name}" to "${name}"`),
    )
    .catch(() => {});
}

export function setTrackColor(trackId: string, track: Track | undefined, color: string | null) {
  api
    .applyEdit(
      [{ op: "set_track_prop", id: trackId, prop: "color", value: color }],
      tr(
        `${track?.name ?? "トラック"} の色を${color ? "変更" : "元に戻す"}`,
        `${color ? "Change" : "Reset"} ${track?.name ?? "track"} color`,
      ),
    )
    .catch(() => {});
}

export function setMasterVolume(v: number) {
  api
    .applyEdit(
      [{ op: "set_master_volume", volume_db: v }],
      tr(`マスター音量を ${v.toFixed(1)} dB に変更`, `Set master volume to ${v.toFixed(1)} dB`),
    )
    .catch(() => {});
}

export const KIND_ICON = { midi: "piano", audio: "audio-lines", bus: "merge" } as const;

/// トラックの種類の表示名(言語の切り替えに追従するよう、使う所で呼ぶ)
export function kindLabel(kind: "midi" | "audio" | "bus"): string {
  return kind === "midi" ? tr("MIDI トラック", "MIDI track") : kind === "audio" ? tr("音声トラック", "Audio track") : tr("バス", "Bus");
}

export function addTrack(project: Project, kind: "midi" | "audio" | "bus" = "midi") {
  const id = newTrackId();
  if (kind === "bus") {
    // バス: 共有リバーブとして使えるよう、リバーブ(ウェット 100%)を挿して作る
    const n = project.tracks.filter((t) => t.kind === "bus").length + 1;
    api
      .applyEdit(
        [
          { op: "add_track", track: { id, name: tr(`リバーブ バス ${n}`, `Reverb bus ${n}`), kind } },
          {
            op: "add_effect",
            track: id,
            effect: { id: newFxId(), type: "builtin", name: "reverb", params: { mix: 1.0 } },
          },
        ],
        tr("バスを追加", "Add bus"),
      )
      .catch(() => {});
    return;
  }
  const name =
    kind === "audio"
      ? tr(`音声 ${project.tracks.filter((t) => t.kind === "audio").length + 1}`, `Audio ${project.tracks.filter((t) => t.kind === "audio").length + 1}`)
      : tr(`トラック ${project.tracks.length + 1}`, `Track ${project.tracks.length + 1}`);
  api
    .applyEdit(
      [{ op: "add_track", track: kind === "midi" ? { id, name, kind, device: builtinDevice("subtractive") } : { id, name, kind } }],
      kind === "audio" ? tr("音声トラックを追加", "Add audio track") : tr("トラックを追加", "Add track"),
    )
    .catch(() => {});
}

/// このバスへ送っているトラックの数
export function sendersOf(project: Project, bus: Track): number {
  return project.tracks.filter((t) => t.sends?.some((s) => s.target === bus.id)).length;
}

/// 音声トラックの空きレーン: 音声ファイルを選んでその小節に音声クリップとして置く
export async function importAudioAt(track: Track, startTick: number) {
  const file = await pickFile({
    title: tr("音声ファイルをクリップとして配置", "Place an audio file as a clip"),
    filters: [{ name: tr("音声(WAV / MP3 / FLAC / OGG / M4A)", "Audio (WAV / MP3 / FLAC / OGG / M4A)"), extensions: ["wav", "mp3", "flac", "ogg", "m4a", "aac"] }],
  });
  if (typeof file !== "string") return;
  await api.importAudioClip(track.id, file, startTick).catch((e) => showError(tr("取り込めませんでした", "Couldn't import"), e));
}

/// MIDI ファイル(.mid)・MusicXML(.musicxml / .xml / .mxl)を読み込む: パートごとに新しいトラックを足す(1 回の undo で戻る)
export async function importMidiFile() {
  const file = await pickFile({
    title: tr("MIDI ファイル・MusicXML を読み込む", "Import a MIDI file / MusicXML"),
    filters: [
      { name: tr("MIDI・MusicXML", "MIDI / MusicXML"), extensions: ["mid", "midi", "musicxml", "mxl", "xml"] },
      { name: "MIDI", extensions: ["mid", "midi"] },
      { name: "MusicXML", extensions: ["musicxml", "mxl", "xml"] },
    ],
  });
  if (typeof file !== "string") return;
  try {
    const r = await api.importMidi({ path: file });
    const extra = r.report && r.report.length ? `。${r.report.join("。")}` : "";
    const extraEn = r.report && r.report.length ? `. ${r.report.join(". ")}` : "";
    showToast(
      "ok",
      tr(
        `読み込みました: トラック ${r.tracks} 本・ノート ${r.notes} 個${r.tempo_set ? "(テンポと拍子も)" : ""}${extra}`,
        `Imported: ${plural(r.tracks, "track")}, ${plural(r.notes, "note")}${r.tempo_set ? " (with tempo and time signature)" : ""}${extraEn}`,
      ),
    );
  } catch (e) {
    showError(tr("読み込めませんでした", "Couldn't import"), e);
  }
}

/// MIDI トラックの空きレーンの `tick` の小節に、空のクリップを置く(4 小節。次のクリップの手前まで)。
/// 置いたら (クリップの ID, 名前)。置く隙間が無ければ null
export async function addClipAt(track: Track, barList: Bar[], tick: number): Promise<{ clipId: string; name: string } | null> {
  const bar = barAtTick(barList, tick);
  let start = bar.tick;
  // 小節頭が前のクリップに食われていたらその終端から
  const covering = track.clips.find((c) => start >= c.start && start < c.start + c.length);
  if (covering) start = covering.start + covering.length;
  // 長さ: 4 小節ぶん(拍子を考慮)。次のクリップの手前まででクランプ
  let length = 0;
  for (let i = bar.index; i < Math.min(bar.index + 4, barList.length); i++) {
    length += barList[i].len;
  }
  if (length === 0) length = 3840 * 4;
  const nextStart = track.clips
    .map((c) => c.start)
    .filter((s) => s >= start + 1)
    .sort((a, b) => a - b)[0];
  if (nextStart !== undefined) length = Math.min(length, nextStart - start);
  if (length < 240) return null; // 置く隙間がない

  const clipId = newClipId();
  const name = tr(`クリップ ${bar.index + 1}`, `Clip ${bar.index + 1}`);
  await api.applyEdit(
    [
      {
        op: "add_clip",
        track: track.id,
        clip: { id: clipId, name, start, length, kind: "midi", notes: [] },
      },
    ],
    tr(`${track.name} の ${bar.index + 1} 小節目にクリップを追加`, `Add clip to ${track.name} at bar ${bar.index + 1}`),
  );
  return { clipId, name };
}

/// クリップの設計の印の説明(計画との関係・手で直した小節・固定の音)
export function clipMarkTitle(st: ClipState): string {
  const out: string[] = [];
  if (st.plan === "ahead")
    out.push(tr("計画が先に進んだ(この計画の今の版から作り直せる)", "The plan has moved ahead (can be rebuilt from the current plan)"));
  if (st.plan === "in_sync") out.push(tr("計画どおり", "Matches the plan"));
  if (st.plan === "missing") out.push(tr("作った元の計画が無い", "The plan it was made from is gone"));
  if (st.edited_bars?.length)
    out.push(
      tr(
        `手で直した小節: ${st.edited_bars.map(([a, b]) => (a === b ? `${a}` : `${a}〜${b}`)).join("・")}(AI の作り直しで残す)`,
        `Hand-edited bars: ${st.edited_bars.map(([a, b]) => (a === b ? `${a}` : `${a}–${b}`)).join(", ")} (kept when AI rebuilds)`,
      ),
    );
  if (st.locked_notes) out.push(tr(`固定の音 ${st.locked_notes} 個(AI は変えない)`, `${st.locked_notes} locked notes (AI won't change them)`));
  return out.join(" / ") + tr("。押すと設計画面のこのパートへ", ". Click to open this part in the Design view");
}
