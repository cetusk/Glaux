import { showError } from "./toast.svelte";
import { tr } from "./i18n.svelte";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AiActivity,
  AppInfo,
  ChatEvent,
  HistorySnapshot,
  MonitorMode,
  PresetInfo,
  SpeakerSim,
  ProjectSnapshot,
  RecentProject,
  Track,
  TransportState,
} from "./types";

export function getProject(): Promise<ProjectSnapshot> {
  return invoke("get_project");
}

/** 履歴の最新側から limit 件(総数は total)。全件は数千件になりうるので既定で絞る */
export function getHistory(limit: number = HISTORY_LIMIT): Promise<HistorySnapshot> {
  return invoke("get_history", { limit });
}

/** 画面で扱う履歴の件数(履歴パネルの表示件数と揃える) */
export const HISTORY_LIMIT = 120;

export function undo(): Promise<{ undone: number; project_version: number }> {
  return invoke("undo");
}

export function redo(): Promise<{ redone: number; project_version: number }> {
  return invoke("redo");
}

/** 履歴の途中のエントリを 1 件だけ取り消す(git revert 相当)。 */
export function revertEntry(
  entryId: string,
): Promise<{ entry_id: string; conflicts: string[]; project_version: number }> {
  return invoke("revert_entry", { entryId });
}

export function appInfo(): Promise<AppInfo> {
  return invoke("app_info");
}

/** 変更通知の中身。save_error は保存に失敗したとき(編集はメモリ上では反映済み)。 */
export interface ProjectChangedEvent {
  project_version: number;
  /** 何が変わったか(`kind` と対象の ID)。空 = 全体が変わった(切り替え・読み直し)か、本体は変わらない操作 */
  changes?: { kind: string; track?: string; clip?: string; id?: string }[];
  save_error?: string;
  /** 曲の中身は変わらず履歴だけが変わった(チェックポイント) */
  history_only?: boolean;
  /** 計画(plans.json)が変わったときの計画の版。曲の中身も版も変わらない(設計画面が読み直すのに使う) */
  plans_version?: number;
}

/** 指定したトラックだけを取得する(見つからない ID は含まれない) */
export function getTracks(ids: string[]): Promise<{ project_version: number; tracks: Track[] }> {
  return invoke("get_tracks", { ids });
}

/** MCP / UI どちらの編集でも発火する。変わった所(changes)を見て取り直す。 */
export function onProjectChanged(cb: (ev: ProjectChangedEvent) => void): Promise<UnlistenFn> {
  return listen<ProjectChangedEvent>("project-changed", (e) => cb(e.payload));
}

/** AI(MCP クライアント)のツール呼び出しの開始/終了。 */
export function onAiActivity(cb: (a: AiActivity) => void): Promise<UnlistenFn> {
  return listen<AiActivity>("ai-activity", (e) => cb(e.payload));
}

// ---- UI からの編集(Command API 経由、author: human) ----

/** Command JSON を適用する。履歴に載り、AI からも get_history で見える。 */
/**
 * 編集を適用する。失敗したら、ここでトーストを出してから投げ直す
 * (呼び出し側の .catch(() => {}) で失敗が黙って消えないように)。
 */
// ---- ドラッグ中の試聴(履歴に載せずに音だけ変える) ----
// 送るのは同時に 1 つだけ。送っている間に来た値は最後の 1 つだけ覚えておき、終わったら送る
let previewBusy: Promise<void> | null = null;
let previewNext: unknown[] | null = null;

function pumpPreview() {
  const commands = previewNext;
  previewNext = null;
  if (!commands) {
    previewBusy = null;
    return;
  }
  previewBusy = invoke<void>("preview_edit", { commands })
    .catch(() => undefined)
    .then(pumpPreview);
}

/** スライダーをドラッグしている間に、その値の音を聴かせる(履歴・プロジェクトは変えない)。 */
export function previewEdit(commands: unknown[]) {
  previewNext = commands;
  if (!previewBusy) pumpPreview();
}

/** 送りかけの試聴を捨て、送っている最中のものが終わるのを待つ(確定の前に。古い試聴が後から上書きしないように) */
async function settlePreview() {
  previewNext = null;
  while (previewBusy) await previewBusy;
}

export async function applyEdit(
  commands: unknown[],
  label: string,
): Promise<{ entry_id: string; project_version: number }> {
  await settlePreview();
  try {
    return await invoke("apply_edit", { commands, label });
  } catch (e) {
    showError(tr(`「${label}」を適用できませんでした`, `Couldn't apply "${label}"`), e);
    throw e;
  }
}

/** 画面のミュート・ソロ: 聴き方なので履歴に積まずに変える(redo の並びも消えない)。set_track_prop の mute / solo だけ */
export async function setListen(commands: unknown[]): Promise<{ project_version: number }> {
  try {
    return await invoke("set_listen", { commands });
  } catch (e) {
    showError(tr("ミュート・ソロを変えられませんでした", "Couldn't change mute / solo"), e);
    throw e;
  }
}

// ---- プロジェクト管理 ----

export function listRecentProjects(): Promise<{
  recent: RecentProject[];
  default_dir: string;
}> {
  return invoke("list_recent_projects");
}

/** 既定の作業フォルダ(新規プロジェクトの作成先)を変更・永続化する。 */
export function setProjectsDir(path: string): Promise<{ default_dir: string }> {
  return invoke("set_projects_dir", { path });
}

/** 現在のプロジェクトを移動 / 名前変更する(省略した方は現状維持)。 */
export function moveProject(
  destParent: string | null,
  newName: string | null,
): Promise<{ path: string; title?: string; project_version?: number; moved: boolean }> {
  return invoke("move_project", { destParent, newName });
}

/** プロジェクトを開く(インプロセス切り替え。MCP・チャット・エンジンは追従する)。 */
export function openProject(
  path: string,
  create = false,
): Promise<{ title: string; path: string; project_version: number }> {
  return invoke("open_project", { path, create });
}

/** フォルダの中の Glaux の曲を探す(フォルダ自体が曲ならそれ 1 つ。2 段下まで)。 */
export function findProjects(dir: string): Promise<{ projects: { path: string; title: string }[] }> {
  return invoke("find_projects", { dir });
}

/** 新しい曲(または移動)で作られるフォルダを前もって調べる。
 *  renamed: 同じ名前があって -2 … が付く / error: そこには作れない(曲のフォルダの中など) */
export type ProjectDirPreview = { path?: string; folder?: string; renamed?: boolean; error?: string };
export function previewProjectDir(
  parentDir: string,
  name: string,
  current: string | null = null,
): Promise<ProjectDirPreview> {
  return invoke("preview_project_dir", { parentDir, name, current });
}

/** `parentDir/name.glaux` に新規プロジェクトを作成して開く。 */
export function createProject(
  parentDir: string,
  name: string,
): Promise<{ title: string; path: string; project_version: number }> {
  return invoke("create_project", { parentDir, name });
}

// ---- 音作りビュー ----

// 追加できるエフェクトのカタログは変わらないので、1 回だけ取って使い回す
let effectCatalog: Promise<import("./types").EffectCatalogEntry[]> | null = null;
function getEffectCatalog(): Promise<import("./types").EffectCatalogEntry[]> {
  effectCatalog ??= invoke<import("./types").EffectCatalogEntry[]>("get_effect_catalog").catch((e) => {
    effectCatalog = null; // 失敗したら次にまた取りにいく
    throw e;
  });
  return effectCatalog;
}

type ParamsWithoutCatalog = Omit<import("./types").TrackParams, "available_effects">;

export async function getTrackParams(trackId: string): Promise<import("./types").TrackParams> {
  const [p, available_effects] = await Promise.all([
    invoke<ParamsWithoutCatalog>("get_track_params", { trackId }),
    getEffectCatalog(),
  ]);
  return { ...p, available_effects };
}

/** マスターバスのエフェクトチェーン(音作りビューのマスターモード用)。 */
export async function getMasterParams(): Promise<import("./types").TrackParams> {
  const [p, available_effects] = await Promise.all([
    invoke<ParamsWithoutCatalog>("get_master_params"),
    getEffectCatalog(),
  ]);
  return { ...p, available_effects };
}

/** WAV を取り込んでトラックの音源を sampler にする(1 undo)。instrument が "granular" なら、音源はそのままで粒の素材にする */
export function importSample(
  trackId: string,
  path: string,
  instrument?: "granular",
): Promise<{ asset_id: string; project_version: number }> {
  return invoke("import_sample", { trackId, path, instrument: instrument ?? null });
}

/** SFZ の鍵盤 1 つの強さの段(rr = ラウンドロビンの数、stack = 重ねて鳴らす録音の数) */
export interface SfzLayer {
  vel_lo: number;
  vel_hi: number;
  rr: number;
  stack: number;
  samples: string[];
}
/** SFZ の楽器の中身(音色エディタの SFZ) */
export interface SfzInspect {
  controls: { cc: number; label: string; default: number }[];
  keys: { key: number; layers: SfzLayer[]; ccs: number[] }[];
  regions: number;
  samples: number;
}
export function sfzInspect(instrument: string, cc: Record<string, number>): Promise<SfzInspect> {
  return invoke("sfz_inspect", { instrument, cc });
}
/** SFZ の鍵盤をその強さで弾いたとき(rr 回目。0 始まり)に鳴る録音の波形。peaks は (最小, 最大) の組 */
export function sfzKeyWave(instrument: string, cc: Record<string, number>, key: number, vel: number, rr: number, buckets: number): Promise<{ peaks: [number, number][]; seconds: number; sample: string }> {
  return invoke("sfz_key_wave", { instrument, cc, key, vel, rr, buckets });
}

/** 音の頭で自動に引く区分の線(長さの割合。使う所の中だけ、区分 1 の頭を除く) */
export function samplerAutoCuts(assetId: string, start: number, end: number): Promise<number[]> {
  return invoke("sampler_auto_cuts", { assetId, start, end });
}

/** 素材の波形: peaks は最小・最大を交互に並べた物(2 × buckets 個)、seconds は長さ */
export function assetPeaks(assetId: string, buckets: number, range?: [number, number]): Promise<{ peaks: number[]; seconds: number }> {
  return invoke("asset_peaks", { assetId, buckets, fromSec: range?.[0] ?? null, toSec: range?.[1] ?? null });
}

/** 畳み込みリバーブの響き(IR)を音声ファイルから取り込み、エフェクトの ir に設定する(1 undo)。trackId が null ならマスター */
export function importIr(trackId: string | null, fxId: string, path: string): Promise<{ asset_id: string }> {
  return invoke("import_ir", { trackId, fxId, path });
}

/** WAV を音声クリップとして音声トラックに置く(履歴 1 件)。 */
export function importAudioClip(
  trackId: string,
  path: string,
  startTick: number,
): Promise<{ clip_id: string; asset_id: string; project_version: number }> {
  return invoke("import_audio_clip", { trackId, path, startTick: Math.max(0, Math.round(startTick)) });
}

/** 参考曲を読み込む: 参考曲のトラック(いちばん上)を作り、音声クリップを曲の頭に置く(履歴 1 件) */
export function importReference(path: string): Promise<{ track_id: string; clip_id: string; project_version: number }> {
  return invoke("import_reference", { path });
}

/** 参考曲の聴き方: off = ふつう / song = 聴き比べの自分の曲 / reference = 参考曲 / both = 重ねる */
export type ReferenceMode = "off" | "song" | "reference" | "both";

export function referenceSetMode(mode: ReferenceMode): Promise<void> {
  return invoke("reference_set_mode", { mode });
}

/** 参考曲と曲の音量をそろえる(同じ範囲の統合ラウドネスを測る。範囲は省略で参考曲のクリップ全体) */
export function referenceMatchLoudness(
  startTick?: number,
  endTick?: number,
): Promise<{
  song_lufs: number | null;
  reference_lufs: number | null;
  song_gain_db: number;
  reference_gain_db: number;
  overlay_gain_db: number;
}> {
  return invoke("reference_match_loudness", { startTick: startTick ?? null, endTick: endTick ?? null });
}

/** 参考曲の骨組み(テンポ・拍子・調・小節ごとの和音・ノリ。summary が言葉の要約) */
export function referenceSkeleton(clipId: string): Promise<{
  summary: string;
  bpm: number | null;
  beats_per_bar: number | null;
  key: { name: string; confidence: number } | null;
  tuning_cents: number;
}> {
  return invoke("reference_skeleton", { clipId });
}

/** 格子の直し(作業場の「格子」の手順) */
export interface GridOptions {
  /** 小節頭を何拍ずらすか(正 = 後ろへ) */
  downbeat_shift: number;
  /** 拍の取り方: 1 = そのまま、2 = 倍の速さ、0.5 = 半分 */
  tempo_factor: number;
  /** 1 小節の拍数(null で推定のまま) */
  beats_per_bar: number | null;
  /** 弱起なし(最初の小節頭を 1 小節目にし、前を切る)。null なら前が静かなら切る */
  no_pickup?: boolean | null;
}

/** 作業場に出す参考曲の骨組みと格子 */
export interface ReconWorkspace {
  duration_sec: number;
  beats: number[];
  bars: { bar: number; time: number; chords: string }[];
  beats_per_bar: number;
  lead_beats: number;
  /** 弱起なしにしたか(最初の小節頭が 1 小節目) */
  no_pickup: boolean;
  audio_start_sec: number;
  tempo: { tick: number; bpm: number }[];
  bpm: number | null;
  key: { name: string; confidence: number } | null;
  tuning_cents: number;
  summary: string;
}

export function reconWorkspace(clipId: string, opts: GridOptions): Promise<ReconWorkspace> {
  return invoke("recon_workspace", { clipId, opts });
}

/** 曲のテンポ・拍子を参考曲の格子にし、参考曲のクリップを合わせる(区間の印も置ける。履歴 1 件) */
export function reconApplyGrid(clipId: string, opts: GridOptions, sections: boolean): Promise<{ project_version: number }> {
  return invoke("recon_apply_grid", { clipId, opts, sections });
}

/** 今の曲への取り込み方(参考曲の小節の範囲・置き始めの小節・移調〈null で今の曲の調へ〉・置き先のトラック) */
export interface ReconImport {
  from_bar: number;
  to_bar: number;
  at_bar: number;
  transpose: number | null;
  track_id: string | null;
}

/** 参考曲の一部のパートを今の曲に取り込む(今の曲のテンポ・拍子は変えない。履歴 1 件) */
export function reconImport(
  clipId: string,
  opts: GridOptions,
  parts: ReconPart[],
  quantize: number,
  imp: ReconImport,
): Promise<{ project_version: number; transpose: number; parts: { name: string; notes: number }[] }> {
  return invoke("recon_import", { clipId, opts, parts, quantize, import: imp });
}

/** 分離ツール(利用者が入れた Demucs)がこの PC にあるか */
export function reconSeparatorAvailable(): Promise<boolean> {
  return invoke("recon_separator_available");
}

/** 参考曲を分離ツールで 4 つ(歌・ドラム・ベース・その他)に分ける(数分かかる)。分けた音のファイルのパス */
/** melody: 旋律を起こす音(歌があれば歌、楽器だけの曲なら「その他」) */
export function reconSeparate(clipId: string): Promise<{ vocals?: string; drums?: string; bass?: string; other?: string; melody?: string }> {
  return invoke("recon_separate", { clipId });
}

/** 起こすパート(source: 省略で参考曲そのもの、パスならそのパートだけを分けた音のファイル) */
export interface ReconPart {
  name: string;
  kind: "melody" | "bass" | "poly" | "drums";
  program: number | null;
  source?: string | null;
}

/** 参考曲からパートを起こしてトラックにする(格子も合わせる。履歴 1 件)。quantize は 16 分の格子へ寄せる強さ(0〜1) */
export function reconTranscribe(
  clipId: string,
  opts: GridOptions,
  parts: ReconPart[],
  quantize: number,
): Promise<{ project_version: number; parts: { name: string; notes: number }[] }> {
  return invoke("recon_transcribe", { clipId, opts, parts, quantize });
}

/** 再生の速さ(1 = ふつう、0.75・0.5。音程は変えない。書き出しには効かない) */
export function transportSetSpeed(speed: number): Promise<void> {
  return invoke("transport_set_speed", { speed });
}

/** 音声クリップの波形ピーク(表示用)。 */
export function clipPeaks(clipId: string, buckets: number): Promise<{ peaks: [number, number][] }> {
  return invoke("clip_peaks", { clipId, buckets });
}

/** 音声クリップの音に似せた内蔵シンセのトラックを作る(つまみは自動で探す。約 20 秒。履歴 1 件)。 */
export function matchClipSound(clipId: string): Promise<{
  track_id: string;
  track_name: string;
  /** 採った音源(subtractive / fm / wavetable) */
  instrument: string;
  reverb: { mix: number; size: number } | null;
  distance: number;
  initial_distance: number;
  verdict: string;
  pitch: number;
  params: Record<string, number | string>;
}> {
  return invoke("match_clip_sound", { clipId });
}

/** 似た音のプリセット探しの候補。distance: 0.15 未満 ほぼ同じ … 0.7 以上 かなり違う */
export interface SimilarPreset {
  id: string;
  name: string;
  category: string;
  distance: number;
  verdict: string;
  clap_similarity: number | null;
}

/** 索引作りの進み具合(対象のプリセット数・索引済み・今回鳴らした数)。 */
export interface PresetIndexProgress {
  total: number;
  indexed: number;
  added: number;
}

/** 音声クリップの音に近い CLAP 音源のプリセットを探す(track_id の CLAP 音源から)。進捗は onPresetIndex。 */
export function findSimilarClapPresets(
  clipId: string,
  trackId: string,
  category: string | null,
  indexSeconds = 90,
): Promise<{
  target: string;
  index: PresetIndexProgress;
  results: SimilarPreset[];
  note?: string;
  clap_note?: string;
}> {
  return invoke("find_similar_clap_presets", { clipId, trackId, category, indexSeconds });
}

export function onPresetIndex(cb: (p: PresetIndexProgress) => void): Promise<UnlistenFn> {
  return listen<PresetIndexProgress>("preset-index", (e) => cb(e.payload));
}

/** CLAP 音源のつまみを音声クリップの音に自動で合わせる(今の音色から出発。履歴 1 件)。 */
export function refineClapParams(
  clipId: string,
  trackId: string,
  maxSeconds = 20,
): Promise<{
  track: string;
  changed: { name: string; path: string; before: number; after: number }[];
  initial_distance: number;
  distance: number;
  verdict: string;
}> {
  return invoke("refine_clap_params", { clipId, trackId, maxSeconds });
}

/** 追加モデルの状態(clap = 音色を言葉で捉えるモデル)。 */
export interface ModelStatus {
  available: boolean;
  path: string;
  bytes: number;
}
export function modelStatus(): Promise<{ clap: ModelStatus }> {
  return invoke("model_status");
}

/** CLAP の音声側モデルを取得する(約 280MB)。進捗は onModelDownload で届く。 */
export function downloadClapModel(): Promise<{ path: string }> {
  return invoke("download_clap_model");
}

export function onModelDownload(cb: (p: { got: number; total: number }) => void): Promise<UnlistenFn> {
  return listen<{ got: number; total: number }>("model-download", (e) => cb(e.payload));
}

/** 音声クリップの元のテンポ・拍子を検出する(先頭 60 秒。Beat This!)。 */
export function detectClipTempo(clipId: string): Promise<{
  bpm: number | null;
  bpm_alternatives: number[];
  beats_per_bar: number | null;
  first_downbeat_sec: number | null;
  tempo_variation: number | null;
  summary: string;
}> {
  return invoke("detect_clip_tempo", { clipId });
}

/** MIDI クリップのノートにスウィングを掛ける(noteIds が空ならクリップ全体。履歴 1 件)。
 *  swing: 0.5 = ストレート、0.667 ≈ 3 連、0.75 = 付点。grid: 480 = 8 分、240 = 16 分 */
export function swingClip(
  clipId: string,
  noteIds: string[] | null,
  grid: number,
  swing: number,
): Promise<{ changed: number }> {
  return invoke("swing_clip", { clipId, noteIds, grid, swing });
}

/** 音声クリップ(単旋律)を譜起こしして MIDI クリップを作る(履歴 1 件)。 */
export function transcribeClip(
  clipId: string,
  destTrackId: string | null = null,
  quantizeTicks = 240,
  /** "melody" = 単旋律(鼻歌・歌・単音)、"poly" = 和音(ピアノ・ギター等) */
  mode: "melody" | "poly" = "melody",
): Promise<{
  clip_id: string;
  track_id: string;
  note_count: number;
  created_track: boolean;
  project_version: number;
}> {
  return invoke("transcribe_clip", { clipId, destTrackId, quantizeTicks, mode });
}

/** 音声クリップをパートに分離して音声トラックに置く(元トラックはミュート)。
 *  builtin = 打楽器 / 音程楽器、demucs = ボーカル / ドラム / ベース / その他(要インストール) */
export function separateClip(
  clipId: string,
  method: "builtin" | "demucs",
): Promise<{ parts: string[]; project_version: number }> {
  return invoke("separate_clip", { clipId, method });
}

// ---- CLAP プラグイン ----

export interface ClapPluginInfo {
  id: string;
  name: string;
  vendor: string;
  version: string;
  path: string;
  instrument: boolean;
  effect: boolean;
}

/** 無料のプラグインの入手の一覧の 1 つ */
export interface PluginStoreEntry {
  id: string;
  name: string;
  /** instrument / effect / both */
  kind: string;
  author: string;
  license: string;
  homepage: string;
  about: string;
  /** すでに入っているかを見分ける名前(プラグインの一覧の名前) */
  names: string[];
  /** この OS で取得して入れられるか(false なら配布ページを開く) */
  install: boolean;
  version: string | null;
  bytes: number | null;
  /** Glaux が入れたか */
  installed: boolean;
}

/** 無料のプラグインの入手の一覧と、入れる場所 */
export function pluginStore(): Promise<{ entries: PluginStoreEntry[]; root: string | null }> {
  return invoke("plugin_store");
}

/** 公式の配布から取得して入れ、探し直す。進捗は onPluginDownload で届く */
export function installPlugin(id: string): Promise<{ dir: string }> {
  return invoke("install_plugin", { id });
}

/** Glaux が入れたプラグインを外す */
export function removePlugin(id: string): Promise<void> {
  return invoke("remove_plugin", { id });
}

/** 配布ページを既定のブラウザで開く */
export function openPluginPage(id: string): Promise<void> {
  return invoke("open_plugin_page", { id });
}

export function onPluginDownload(cb: (p: { id: string; got: number; total: number }) => void): Promise<UnlistenFn> {
  return listen<{ id: string; got: number; total: number }>("plugin-download", (e) => cb(e.payload));
}

/** インストール済みの CLAP プラグイン(rescan で探し直す)。dirs は探している場所 */
export function clapPlugins(rescan = false): Promise<{ plugins: ClapPluginInfo[]; dirs: string[] }> {
  return invoke("clap_plugins", { rescan });
}

/** CLAP プラグイン自身の画面を開く(開いていれば前面へ)。今は Windows のみ。
 *  音源はトラック ID、エフェクトは fxId を渡す(trackId は null でよい) */
export function clapOpenGui(trackId: string | null, fxId: string | null = null): Promise<void> {
  return invoke("clap_open_gui", { trackId, fxId });
}

export function clapCloseGui(trackId: string | null, fxId: string | null = null): Promise<void> {
  return invoke("clap_close_gui", { trackId, fxId });
}

export interface ClapPreset {
  id: string;
  name: string;
  category: string;
  collection: string;
  factory: boolean;
  creators: string[];
  description: string;
  features: string[];
}

/** トラックの CLAP プラグインのプリセット一覧(rescan で探し直す)。 */
export function clapPresets(
  trackId: string | null,
  rescan = false,
  fxId: string | null = null,
): Promise<{
  presets: ClapPreset[];
  categories: { name: string; count: number }[];
  current_preset: string | null;
  total: number;
}> {
  return invoke("clap_presets", { trackId, fxId, rescan });
}

/** CLAP プラグインのトラックにプリセットを読み込む(履歴 1 件)。 */
export function clapLoadPreset(
  trackId: string | null,
  preset: string,
  fxId: string | null = null,
): Promise<{ preset: string }> {
  return invoke("clap_load_preset", { trackId, fxId, preset });
}

/** トラックの CLAP プラグインの今の設定をプロジェクトに保存する。 */
export function clapSaveState(
  trackId: string | null,
  fxId: string | null = null,
): Promise<{ changed: boolean }> {
  return invoke("clap_save_state", { trackId, fxId });
}

// ---- 録音 ----

/** 録音を開始する(再生も同時に始まる)。カウントイン後の位置にクリップが置かれる。 */
export function recordStart(opts: {
  countInBars: number;
  latencyMs: number;
  metronome: boolean;
  /** 入力が 2 ch 以上ならステレオで録る */
  stereo?: boolean;
}): Promise<{ clip_start: number; count_in_ticks: number }> {
  return invoke("record_start", opts);
}

export function transportSetMetronome(on: boolean): Promise<void> {
  return invoke("transport_set_metronome", { on });
}

/** 聴き方とクロスフィード(出力デバイスへの音だけ。書き出しには入らない) */
export function transportSetMonitor(mode: MonitorMode, crossfeed: boolean): Promise<void> {
  return invoke("transport_set_monitor", { mode, crossfeed });
}

/** マスターの直近のスペクトル(1/3 オクターブ。帯域の中心 Hz と dB) */
export function transportSpectrum(): Promise<{ bands: number[]; db: number[] }> {
  return invoke("transport_spectrum");
}

/** ラウドネスメーターの統合値と True Peak の最大を測り直す */
export function transportResetLoudness(): Promise<void> {
  return invoke("transport_reset_loudness");
}

/** 小さなスピーカーのシミュレーション(出力デバイスへの音だけ。書き出しには入らない) */
export function transportSetSpeaker(speaker: SpeakerSim): Promise<void> {
  return invoke("transport_set_speaker", { speaker });
}

/** アプリから鳴る音の音量(dB)。聴く音量だけで、曲・メーター・書き出しには入らない */
export function setOutputVolume(db: number): Promise<void> {
  return invoke("transport_set_output_volume", { db });
}

/** ゴニオメーターの点(古い順の [左, 右]) */
export function transportScope(): Promise<[number, number][]> {
  return invoke("transport_scope");
}

/** 録音を止めて音声クリップとして配置する。trackId 省略で最初の音声トラック(無ければ新設)。
 *  autoGain で一番大きい所が -6dB になるようクリップの音量を上げる。 */
export function recordStop(
  trackId: string | null = null,
  autoGain = true,
): Promise<{
  clip_id: string;
  track_id: string;
  seconds: number;
  clipped: number;
  dropped: number;
  /** 入力の音切れの回数(パソコンが忙しく、入力を取りこぼした) */
  xruns: number;
  gain_db: number;
  project_version: number;
}> {
  return invoke("record_stop", { trackId, autoGain });
}

// ---- MIDI キーボード ----

/** MIDI 入力ポートの一覧と接続中のポート。 */
export function midiInputs(): Promise<{ inputs: string[]; current: string | null }> {
  return invoke("midi_inputs");
}

/** MIDI 入力に接続する(null = 切断)。 */
export function setMidiInput(name: string | null): Promise<{ current: string | null }> {
  return invoke("set_midi_input", { name: name || null });
}

/** MIDI キーボードで鳴らすトラック(null = 既定音色)。 */
export function setLiveTarget(trackId: string | null): Promise<void> {
  return invoke("set_live_target", { trackId });
}

/** MIDI 録音を開始する(再生も同時に始まる)。 */
export function midiRecordStart(opts: {
  countInBars: number;
  metronome: boolean;
}): Promise<{ clip_start: number; count_in_ticks: number }> {
  return invoke("midi_record_start", opts);
}

/** MIDI 録音を止めてクリップとして置く。quantizeTicks > 0 で開始位置をグリッドに丸める。 */
export function midiRecordStop(
  trackId: string | null,
  quantizeTicks = 0,
): Promise<{ clip_id: string; track_id: string; notes: number; project_version: number }> {
  return invoke("midi_record_stop", { trackId, quantizeTicks });
}

// ---- オーディオデバイス ----

export interface AudioDevices {
  outputs: string[];
  inputs: string[];
  default_output: string | null;
  default_input: string | null;
  current_output: string | null;
  current_input: string | null;
  sample_rate: number;
  /** 出力バッファ(フレーム): 希望・実際に指定できた大きさ(null = OS 任せ)・デバイスの範囲・直前のブロック */
  buffer?: { requested: number; applied: number | null; min: number | null; max: number | null; block: number } | null;
}

export function audioDevices(): Promise<AudioDevices> {
  return invoke("audio_devices");
}

/** 出力バッファの大きさ(フレーム、0 = 既定の 1024)を変えて開き直す */
export function setBufferSize(frames: number): Promise<{ requested: number; applied: number | null }> {
  return invoke("set_buffer_size", { frames });
}

/** 出力デバイスを切り替える(null = OS の既定)。 */
export function setOutputDevice(
  name: string | null,
): Promise<{ current_output: string; sample_rate: number }> {
  return invoke("set_output_device", { name: name || null });
}

/** 録音に使う入力デバイス(null = OS の既定)。 */
export function setInputDevice(name: string | null): Promise<{ current_input: string | null }> {
  return invoke("set_input_device", { name: name || null });
}

/** 入力テスト(録音せずに入力レベルだけ測る)。 */
export function inputMonitor(on: boolean): Promise<void> {
  return invoke("input_monitor", { on });
}

/** 入力テストの音を出力で聴く(自分の声の確認。入力テスト中だけ鳴る) */
export function inputListen(on: boolean): Promise<void> {
  return invoke("input_listen", { on });
}

/** 遅延の較正: メトロノームだけ鳴らして 1 小節のカウントイン後の 8 拍を録る。 */
export function calibrateStart(): Promise<{ total_secs: number; count_in_secs: number; beats: number }> {
  return invoke("calibrate_start");
}

export function calibrateStop(): Promise<{
  latency_ms: number;
  spread_ms: number;
  detected: number;
  beats: number;
}> {
  return invoke("calibrate_stop");
}

// ---- SoundFont ----

export interface SfzPack {
  id: string;
  name: string;
  kind: string;
  author: string;
  license: string;
  approx_mb: number;
  installed: boolean;
  instruments: string[];
  /** 選んだときに付ける調整つまみ(CC 番号 → 値) */
  cc?: Record<string, number>;
}

/** 無料の SFZ 音源を取得する。進捗は `sfz-download` イベント({id, got, total}) */
export function downloadSfzPack(id: string): Promise<{ instruments: string[] }> {
  return invoke("download_sfz_pack", { id });
}

/** トラックを複製して元のすぐ下に置く(CLAP の状態・エフェクトのつながり・変調・マクロも写す) */
export function duplicateTrack(trackId: string): Promise<{ entry_id: string; project_version: number }> {
  return invoke("duplicate_track", { trackId });
}

export function listSoundfonts(): Promise<{ dir: string; files: string[]; sfz_dir?: string; sfz?: string[]; packs?: SfzPack[] }> {
  return invoke("list_soundfonts");
}

export function listSoundfontPresets(
  file: string,
): Promise<{ file: string; presets: { bank: number; preset: number; name: string }[] }> {
  return invoke("list_soundfont_presets", { file });
}

/** はじめの確認: 音の出力(デバイス名。使えなければ null)・AI の CLI のパス(無ければ null)・SoundFont のライブラリ */
export interface SetupStatus {
  audio_output: string | null;
  claude: string | null;
  codex: string | null;
  soundfont: { dir: string; files: string[]; download_file: string; download_bytes: number };
}

export function setupStatus(): Promise<SetupStatus> {
  return invoke("setup_status");
}

/** GM 音源一式の SoundFont(GeneralUser GS)を取得する。進捗は onSoundFontDownload で届く */
export function downloadSoundFont(): Promise<{ path: string }> {
  return invoke("download_soundfont");
}

export function onSoundFontDownload(cb: (p: { got: number; total: number }) => void): Promise<UnlistenFn> {
  return listen<{ got: number; total: number }>("soundfont-download", (e) => cb(e.payload));
}

/** 起動時に GM 音源を自動で入れ終えた */
export function onSoundFontInstalled(cb: () => void): Promise<UnlistenFn> {
  return listen("soundfont-installed", () => cb());
}

export function onSfzDownload(cb: (p: { id: string; got: number; total: number }) => void): Promise<UnlistenFn> {
  return listen<{ id: string; got: number; total: number }>("sfz-download", (e) => cb(e.payload));
}

/** 同梱のデモ曲を曲のフォルダに写して開く */
export function openDemoSong(): Promise<{ title: string; project_version: number; path: string }> {
  return invoke("open_demo_song");
}

/** .sf2 をライブラリフォルダへコピーして登録する。 */
export function addSoundfont(path: string): Promise<{ file: string }> {
  return invoke("add_soundfont", { path });
}

// ---- 音色プリセット ----

export function listPresets(): Promise<{ presets: PresetInfo[] }> {
  return invoke("list_presets");
}

/** トラックの現在の音(音源 + エフェクト)をプリセット保存する。 */
export function savePreset(
  trackId: string,
  name: string,
  overwrite = false,
): Promise<{ saved: string }> {
  return invoke("save_preset", { trackId, name, overwrite });
}

/** プリセットをトラックに適用する(音源差し替え + エフェクト置換。1 undo)。 */
export function loadPreset(trackId: string, name: string): Promise<{ applied: string }> {
  return invoke("load_preset", { trackId, name });
}

// ---- エフェクトのプリセット(エフェクト 1 つ分) ----

export interface FxPresetInfo {
  name: string;
  /** 内蔵エフェクト名(eq / compressor …)。CLAP は "clap" */
  kind: string;
  plugin_id?: string;
  note?: string;
  /** 保存元のトラック名 */
  origin?: string;
  created: string;
}

export function listFxPresets(): Promise<{ presets: FxPresetInfo[] }> {
  return invoke("list_fx_presets");
}

/** エフェクト 1 つを名前を付けて保存する。target はトラック ID か "master" */
export function saveFxPreset(
  target: string,
  fxId: string,
  name: string,
  note: string | null = null,
  overwrite = false,
): Promise<{ saved: string }> {
  return invoke("save_fx_preset", { target, fxId, name, note, overwrite });
}

/** エフェクトのプリセットを足す。parked なら、つながずに pos に置く。split なら、その線(from → to)の間に入れる */
export function applyFxPreset(
  target: string,
  name: string,
  opts: { index?: number | null; parked?: boolean; pos?: [number, number] | null; split?: [string, string] | null } = {},
): Promise<{ fx_id: string }> {
  return invoke("apply_fx_preset", {
    target,
    name,
    index: opts.index ?? null,
    parked: opts.parked ?? false,
    pos: opts.pos ?? null,
    split: opts.split ?? null,
  });
}

export function deleteFxPreset(name: string): Promise<{ deleted: string }> {
  return invoke("delete_fx_preset", { name });
}

// ---- トランスポート(再生) ----

export function transportState(): Promise<TransportState> {
  return invoke("transport_state");
}

export function transportPlay(): Promise<void> {
  return invoke("transport_play");
}

export function transportPause(): Promise<void> {
  return invoke("transport_pause");
}

export function transportStop(): Promise<void> {
  return invoke("transport_stop");
}

export function transportSeek(tick: number): Promise<void> {
  return invoke("transport_seek", { tick: Math.max(0, Math.round(tick)) });
}

/** ループ区間を設定する(tick)。 */
export function transportSetLoop(startTick: number, endTick: number): Promise<void> {
  return invoke("transport_set_loop", {
    startTick: Math.max(0, Math.round(startTick)),
    endTick: Math.max(0, Math.round(endTick)),
  });
}

export function transportClearLoop(): Promise<void> {
  return invoke("transport_clear_loop");
}

/** 音量をそろえた A/B の聴き比べの用意の結果 */
export interface AbInfo {
  /** それぞれの統合ラウドネス(LUFS。短すぎ・無音なら null) */
  lufs_a: number | null;
  lufs_b: number | null;
  /** そろえるために掛けた量(dB。0 か負) */
  gain_a_db: number;
  gain_b_db: number;
  from_secs: number;
  to_secs: number;
  /** 比べる編集の数 */
  edits_compared: number;
  /** 実際に書き出した範囲の終わり(長すぎる範囲は詰める) */
  end_tick: number;
  /** 範囲の中で A と B の音が違い始める位置(範囲の頭からの秒)。同じなら null(切り替えても違いが聞こえない) */
  first_diff_secs?: number | null;
}

/** 聴き比べを用意する: A = その編集の前、B = 今。同じ範囲を書き出して音量をそろえ、B を鳴らす状態にする */
export function abPrepare(beforeEntry: string, startTick: number, endTick: number): Promise<AbInfo> {
  return invoke("ab_prepare", {
    beforeEntry,
    startTick: Math.max(0, Math.round(startTick)),
    endTick: Math.max(0, Math.round(endTick)),
  });
}

/** 聴き比べでどちらを鳴らすか */
export function abSetSide(side: "a" | "b" | "c" | "d" | "e" | "off"): Promise<void> {
  return invoke("ab_set_side", { side });
}

/** 聴き比べを終える */
export function abClear(): Promise<void> {
  return invoke("ab_clear");
}

/** ノートを 1 音だけ試聴する(そのトラックの音源・音量・パンで鳴る)。 */
// ---- 音色エディタのウェーブテーブル(作り方の手順 → 絵の材料・試聴・確定) ----
export interface WtFrame {
  wave: number[];
  amps: number[];
  /** ラジアン。x = Σ a·sin(2πhx + φ) */
  phases: number[];
}
export interface WtView {
  frames: number;
  pre_frames: number;
  stack: number[][];
  pre: WtFrame;
  post: WtFrame;
}
export function wavetableView(trackId: string, recipe: unknown, posOut: number, posSrc: number): Promise<WtView> {
  return invoke("wavetable_view", { trackId, recipe, posOut, posSrc, stack: 40, stackPoints: 128, points: 512, harm: 256 });
}
export function wavetablePreview(trackId: string, recipe: unknown): Promise<unknown[]> {
  return invoke("wavetable_preview", { trackId, recipe });
}
export function wavetableCommit(trackId: string, recipe: unknown, label: string, extra: unknown[]): Promise<{ entry_id: string; asset_id: string }> {
  return invoke("wavetable_commit", { trackId, recipe, label, extra });
}
export function wavetableRecipe(assetId: string): Promise<any> {
  return invoke("wavetable_recipe", { assetId });
}
export function wavetableThumbs(trackId: string, sources: unknown[]): Promise<(number[][] | null)[]> {
  return invoke("wavetable_thumbs", { trackId, sources, points: 64 });
}
export function wavetableLibrary(): Promise<{ name: string; note?: string; frames?: number }[]> {
  return invoke("wavetable_library");
}
export function wavetableLibrarySave(trackId: string, name: string, recipe: unknown): Promise<{ saved: string }> {
  return invoke("wavetable_library_save", { trackId, name, recipe });
}
export function wavetableExport(trackId: string, recipe: unknown, path: string): Promise<{ path: string }> {
  return invoke("wavetable_export", { trackId, recipe, path });
}

/** 音色エディタの鍵盤: 押している間だけ鳴らす(離すのは liveNoteOff)。停止中でも鳴る */
export function liveNoteOn(trackId: string, pitch: number, vel = 100): Promise<void> {
  return invoke("live_note_on", { trackId, pitch, vel });
}

export function liveNoteOff(pitch: number): Promise<void> {
  return invoke("live_note_off", { pitch });
}

/** 押している音を全部離す */
export function liveAllOff(): Promise<void> {
  return invoke("live_all_off");
}

/** 1 音を鳴らして、鍵盤の高さの倍数ごとの強さ(線形)を測る。times = いちばん大きくなった所から何秒後か。
 *  device を渡すとその音源で(ドラッグ中の値)。トラックのエフェクトは通さない */
export function renderNoteHarmonics(
  trackId: string,
  pitch: number,
  times: number[],
  opts: { device?: unknown; vel?: number; step?: number; maxMult?: number } = {},
): Promise<number[][]> {
  return invoke("render_note_harmonics", {
    trackId,
    device: opts.device ?? null,
    pitch,
    vel: opts.vel ?? 100,
    times,
    step: opts.step ?? 0.5,
    maxMult: opts.maxMult ?? 40,
  });
}

export function previewNote(trackId: string, pitch: number): Promise<void> {
  return invoke("preview_note", { trackId, pitch });
}

// ---- チャット(UI → AI 指示) ----

/** 指示を送る。進捗は onChatEvent で届く。 */
/** 指示を送る。provider は "claude"(Claude Code)か "codex"(Codex CLI = GPT)。 */
export function sendChat(
  prompt: string,
  model: string | null = null,
  provider: "claude" | "codex" = "claude",
  effort: string | null = null,
  language: "ja" | "en" = "ja",
): Promise<void> {
  return invoke("send_chat", { prompt, model: model || null, provider, effort: effort || null, language });
}

export function cancelChat(): Promise<void> {
  return invoke("cancel_chat");
}

/** 書き出しの依頼(glaux-mcp の ExportRequest と同じ) */
export interface ExportRequest {
  path?: string;
  sample_rate?: number;
  bits?: number;
  /** "wav"(既定)か "flac"(16 / 24bit のみ) */
  format?: "wav" | "flac";
  /** 16bit のノイズシェーピング(既定 true) */
  noise_shaping?: boolean;
  start_tick?: number;
  end_tick?: number;
  loudness_lufs?: number;
  stems?: boolean;
}

/** 書き出す。ミックスなら path と測定値、ステムなら stems と folder */
export function exportAudio(request: ExportRequest): Promise<{
  path?: string;
  seconds?: number;
  lufs?: number;
  peak_db?: number;
  true_peak_db?: number;
  plr_db?: number;
  gain_db?: number;
  limiter_db?: number;
  streaming?: { service: string; target_lufs: number; gain_db: number; result_lufs: number; note?: string }[];
  stems?: { track: string; path: string }[];
  folder?: string;
}> {
  return invoke("export_audio", { request });
}

/** MIDI ファイルを読み込む(パートごとに新しいトラック。テンポはクリップが無い曲のときだけ移す) */
export function importMidi(request: {
  path: string;
  start_tick?: number;
  set_tempo?: boolean;
  soundfont?: string;
}): Promise<{ tracks: number; notes: number; tempo_set: boolean; report?: string[] }> {
  return invoke("import_midi", { request });
}

/** MIDI ファイルに書き出す(path 省略でプロジェクトの export/) */
export function exportMidi(path?: string): Promise<{ path: string; tracks: number }> {
  return invoke("export_midi", { path: path ?? null });
}

/** ゲーム用(Glaux for Godot)に書き出した結果 */
export type GameExport = {
  dir: string;
  frozen: string[];
  soundfonts: string[];
  missing_soundfonts: string[];
  unbaked_effects: string[];
  solo_cleared: string[];
  /** 外した参考曲のトラック(ゲームでは鳴らさず、音声も写さない) */
  references_removed?: string[];
  plugins: string[];
};

/** 効果音用の曲を作った結果 */
export type SfxSong = {
  dir: string;
  tracks: string[];
  skipped: string[];
  removed_effects: string[];
  soundfonts: string[];
  missing_soundfonts: string[];
};

/** 効果音用の曲を作る(今の曲のトラックと棚のプリセットを並べる。dest 省略でプロジェクトの export/) */
export function makeSfxSong(trackIds: string[], presets: string[], name: string, dest?: string): Promise<SfxSong> {
  return invoke("make_sfx_song", { trackIds, presets, name, dest: dest ?? null });
}

/** ゲーム用に書き出す(CLAP・SFZ を音声に、SoundFont を同梱、ソロを外す。dest 省略でプロジェクトの export/) */
export function exportForGame(dest?: string): Promise<GameExport> {
  return invoke("export_for_game", { dest: dest ?? null });
}

/** キーと小節ごとのコード(ノートからの推定)とスケールの音 */
export function harmony(): Promise<import("./harmony.svelte").HarmonyView> {
  return invoke("harmony");
}

/** 記録(ログ)の詳しさと記録のファイル。env は環境変数 GLAUX_LOG で決めているときの中身(そちらが優先) */
export function getLogLevel(): Promise<{ level: LogLevel; path: string; env: string | null }> {
  return invoke("get_log_level");
}

/** 記録の詳しさを変える(すぐ効き、次の起動でも使う) */
export function setLogLevel(level: LogLevel): Promise<void> {
  return invoke("set_log_level", { level });
}

export type LogLevel = "error" | "warn" | "info" | "debug";

/** AI にいつも守らせる指示(設定フォルダの instructions.md) */
export function getStandingInstructions(): Promise<{ text: string; path: string; max: number }> {
  return invoke("get_standing_instructions");
}

/** AI にいつも守らせる指示を書く(空なら消す) */
export function setStandingInstructions(text: string): Promise<void> {
  return invoke("set_standing_instructions", { text });
}

/** パートどうしの音のぶつかり(聞こえているトラックだけ。出てくる所すべて付き) */
export function checkClashes(): Promise<import("./clash.svelte").ClashView> {
  return invoke("check_clashes");
}

/** トラックを音声にする(フリーズ)。直後に音声トラックができ、元はミュートされる */
export function bounceTrack(trackId: string): Promise<{ entry_id: string; new_track: string; seconds: number }> {
  return invoke("bounce_track", { trackId });
}

/** チャットの 1 ターン(`since` より後)で AI が行った編集の要約。entry_ids が件数 */
export function turnChanges(since: string | null): Promise<{
  entry_ids: string[];
  clips: { clip_id: string; added_ids?: string[]; changed_ids?: string[] }[];
}> {
  return invoke("turn_changes", { since });
}

/** チャットの 1 ターンで AI が行った編集を取り消す(新しい順に revert) */
export function revertTurn(since: string | null): Promise<{ reverted: number; conflicts: string[] }> {
  return invoke("revert_turn", { since });
}

/** 送った指示を直して送り直す前に、戻る編集の数を数える(song = 曲、plan = 計画、human = そのうち人の編集) */
export function chatRewindPreview(since: string | null, afterMs: number): Promise<{ song: number; plan: number; human: number }> {
  return invoke("chat_rewind_preview", { since, afterMs });
}

/** 送った指示より後の編集を(人の分も)取り消し、会話もその前まで戻す。chainEnd が無ければ新しい会話になる */
export function chatRewind(since: string | null, afterMs: number, chainEnd: string | null): Promise<{ song: number; plan: number }> {
  return invoke("chat_rewind", { since, afterMs, chainEnd });
}

/** 会話をリセットする(次の送信が新しいセッションになる)。 */
export function resetChat(): Promise<void> {
  return invoke("reset_chat");
}

/** 画面の会話ログ(JSON の文字列)と、それを読んだプロジェクトのフォルダ */
export function loadChatLog(): Promise<{ dir: string; log: string | null }> {
  return invoke("load_chat_log");
}

/** 会話ログを保存する。dir は loadChatLog で受け取ったもの(プロジェクトが変わっていたら保存されない) */
export function saveChatLog(dir: string, log: string): Promise<boolean> {
  return invoke("save_chat_log", { dir, log });
}

/** AI から人への質問(MCP の ask_user) */
export interface AiQuestion {
  id: string;
  why?: string | null;
  questions: { question: string; header: string; multi: boolean; options: { label: string; description?: string | null }[] }[];
}

export function onAiQuestion(cb: (q: AiQuestion) => void): Promise<UnlistenFn> {
  return listen<AiQuestion>("ai-question", (e) => cb(e.payload));
}

export function onChatEvent(cb: (e: ChatEvent) => void): Promise<UnlistenFn> {
  return listen<ChatEvent>("chat-event", (e) => cb(e.payload));
}
