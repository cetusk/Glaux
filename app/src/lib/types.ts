// project.json / MCP レスポンスに対応する型(glaux-core の serde 形と一致させる)

export type Articulation =
  | "normal"
  | "palm_mute"
  | "staccato"
  | "accent"
  | "vibrato"
  | "bend"
  | "legato"
  | "portamento";

export interface Note {
  id: string;
  pos: number;
  dur: number;
  pitch: number;
  vel: number;
  /** 奏法。省略 = normal */
  articulation?: Articulation;
  /** 連続ピッチカーブ(ノート先頭からの相対 tick, セント)。省略 = なし */
  pitch_curve?: { tick: number; cents: number }[];
  /** ポルタメントで滑る時間(ms)。省略 = トラックの glide_ms */
  glide_ms?: number;
}

export interface MidiClip {
  id: string;
  name: string;
  start: number;
  length: number;
  kind: "midi";
  notes: Note[];
  /** ループ(繰り返し)。JSON のキーは "loop" */
  loop?: boolean;
  /** ループ時に繰り返す長さ(クリップ先頭から、tick) */
  loop_len?: number;
}

export interface AudioClip {
  id: string;
  name: string;
  start: number;
  length: number;
  kind: "audio";
  asset: string;
  offset_samples?: number;
  gain_db?: number;
  /** フェードイン / アウトの長さ(ミリ秒) */
  fade_in_ms?: number;
  fade_out_ms?: number;
  /** テンポ追従(音程を保ったまま伸縮)。original_bpm = 素材を演奏したテンポ */
  stretch?: { mode: "none" } | { mode: "follow"; original_bpm: number };
}

export type Clip = MidiClip | AudioClip;

export interface AutomationPoint {
  tick: number;
  value: number;
  curve?: "linear" | "hold" | "exponential";
}

export interface AutomationLane {
  target: string;
  points: AutomationPoint[];
}

/** センド: トラックの音を分けてバスへ送る */
export interface TrackSend {
  target: string;
  level_db: number;
  pre_fader?: boolean;
}

export interface Track {
  id: string;
  name: string;
  /** bus = バス(リターン)。クリップを持たず、他のトラックのセンドを受ける */
  kind: "midi" | "audio" | "bus";
  sends?: TrackSend[];
  /** ポルタメントで滑る時間(ms)。省略 = 150 */
  glide_ms?: number;
  /** レガートのつなぎ目の長さ(ms)。省略 = 30 */
  legato_ms?: number;
  color?: string;
  mute: boolean;
  solo: boolean;
  volume_db: number;
  pan: number;
  device?: {
    type?: string;
    name?: string;
    /** type: "clap" のときのプラグイン ID と状態(base64) */
    plugin_id?: string;
    state?: string;
    params?: Record<string, unknown>;
  } | null;
  effects: ProjectEffect[];
  /** エフェクトのつながり(ノード表示の線)。無ければ並び順の直列 */
  fx_links?: FxLink[] | null;
  /** ノード表示での入力と出口の置き場所(無ければ自動) */
  fx_io_pos?: FxIoPos | null;
  clips: Clip[];
  automation: AutomationLane[];
}

export interface FxIoPos {
  input: [number, number];
  output: [number, number];
}

/** エフェクトの線 1 本。端は "in"(音源・受けた音)/ "out"(音量・パンへ)/ エフェクト ID */
export interface FxLink {
  from: string;
  to: string;
  /** この線を通る音の量(dB)。省略 = 0 */
  gain_db?: number;
}

export interface Project {
  format: string;
  version: number;
  ppq: number;
  meta: { title: string; created: string };
  tempo_map: { tick: number; bpm: number }[];
  time_sig_map: { tick: number; num: number; den: number }[];
  tracks: Track[];
  master: { volume_db: number; effects: ProjectEffect[]; fx_links?: FxLink[] | null; fx_io_pos?: FxIoPos | null; automation?: AutomationLane[] };
  assets: Record<string, unknown>;
  /** 曲の構成マーカー(tick 昇順)。省略 = なし */
  sections?: { tick: number; name: string }[];
}

export type Author =
  | { kind: "human" }
  | { kind: "ai"; model: string }
  | { kind: "system" };

export interface EntrySummary {
  id: string;
  author: Author;
  timestamp: string;
  label: string;
  targets: { kind: string; id?: string }[];
  reverts?: string;
}

export interface ProjectSnapshot {
  project_version: number;
  project: Project;
}

export interface HistorySnapshot {
  project_version: number;
  /** 履歴の総数(entries は最新側から limit 件) */
  total: number;
  entries: EntrySummary[];
  /** やり直せる(取り消した)編集。次にやり直すものが先頭 */
  redoable: EntrySummary[];
}

export interface AppInfo {
  project_dir: string;
  mcp_url: string;
}

export interface RecentProject {
  path: string;
  title: string;
  last_opened: string;
  exists: boolean;
  current: boolean;
}

// ---- 音作りビュー(get_track_params の返り値) ----

export type ParamRangeView =
  | { kind: "float"; min: number; max: number; default: number; skew?: number }
  | { kind: "int"; min: number; max: number; default: number }
  | { kind: "bool"; default: boolean }
  | { kind: "enum"; default: string; choices: string[] };

export interface ParamView {
  name: string;
  display_name: string;
  unit?: string | null;
  range: ParamRangeView;
  description: string;
  /** set_param に渡すパス(device/... or fx/<id>/...) */
  path: string;
  current: unknown;
  /** CLAP プラグインのつまみ: プラグイン自身の表示(例 "-12.0 dB")。まだ分からなければ null */
  current_text?: string | null;
}

/** プロジェクトの中のエフェクト(project.json のまま) */
export interface ProjectEffect {
  id: string;
  /** "builtin" / "clap" */
  type: string;
  /** 内蔵エフェクト名 */
  name?: string;
  plugin_id?: string;
  bypass?: boolean;
  params?: Record<string, unknown>;
  /** 表示名(ユーザーが付けた名前) */
  label?: string;
  /** 線から外してある(つながりの表が無いトラックだけで使う。音は通らない) */
  parked?: boolean;
  note?: string;
  /** ノード表示での位置 [x, y] */
  pos?: [number, number];
}

export interface EffectView {
  id: string;
  /** 内蔵エフェクト名。CLAP プラグインは "clap" */
  name: string;
  bypass: boolean;
  /** 表示名・外してあるか・メモ・ノード表示での位置 */
  label?: string;
  parked?: boolean;
  /** 入力から出口まで線でたどれて鳴っているか */
  sounding?: boolean;
  note?: string;
  pos?: [number, number];
  params: ParamView[];
  /** CLAP エフェクトのとき */
  plugin_id?: string;
  plugin_name?: string | null;
  /** プラグインが見つからない(この PC に入っていない。鳴らすときは素通し) */
  missing?: boolean;
  /** つまみの総数(一覧は最大 64 個) */
  param_total?: number;
}

export interface EffectCatalogEntry {
  name: string;
  description: string;
  params: unknown[];
}

export interface TrackParams {
  track_id: string;
  device: { name: string; is_default_fallback: boolean };
  params: ParamView[];
  effects: EffectView[];
  available_effects: EffectCatalogEntry[];
  project_version: number;
}

export interface PresetInfo {
  name: string;
  description?: string;
  instrument: string;
  effects: string[];
  created: string;
}

export interface AiActivity {
  tool: string;
  busy: boolean;
}

export interface TransportState {
  available: boolean;
  playing: boolean;
  /** 録音中(入力デバイス → 音声クリップ) */
  recording?: boolean;
  /** MIDI 録音中(recording も true になる) */
  midi_recording?: boolean;
  /** 最後に MIDI を受信してからの経過 ms(未受信なら null) */
  midi_idle_ms?: number | null;
  /** メトロノーム ON */
  metronome?: boolean;
  /** 入力レベルのピーク(dBFS。前回の取得から)。録音も入力テストもしていなければ null */
  input_peak_db?: number | null;
  /** 入力テスト中 */
  input_monitor?: boolean;
  /** オーディオ処理の負荷(直近の平均・最大 % と起動からの累計回数) */
  dsp?: {
    avg_pct: number;
    max_pct: number;
    overruns: number;
    late: number;
    swaps: number;
    /** OS のオーディオが知らせてきた音切れ(対応している環境だけ) */
    xruns?: number;
    /** OS がリアルタイム優先度を認めなかった */
    realtime_denied?: boolean;
  };
  tick: number;
  /** ループ区間 [開始tick, 終了tick]。null ならループなし */
  loop?: [number, number] | null;
  /** ミキサーのメーター: トラック(プロジェクトの並び)とマスターの直近のピーク(dBFS、無音は -120)。
   *  correlation はマスターの左右の相関(-1..1、ほぼ無音なら null) */
  levels?: {
    tracks: number[];
    master: number;
    correlation?: number | null;
    /** トラックごと・マスターの処理の重さ(%。処理時間 / 音の長さ。前回の問い合わせから) */
    loads?: number[];
    master_load?: number;
  };
  /** 聴き方(出力デバイスへの音だけ。書き出しには入らない) */
  monitor?: { mode: MonitorMode; crossfeed: boolean; speaker?: SpeakerSim };
  /** マスターのラウドネス(LUFS。瞬時 / 短期 / 統合)と True Peak(dBTP。測り始めてからの最大 / 直近)。測れなければ null */
  loudness?: {
    momentary: number | null;
    short_term: number | null;
    integrated: number | null;
    true_peak_max: number | null;
    true_peak: number | null;
  };
}

/** 聴き方: そのまま / 左右を足す / 左右の差だけ / 左右を入れ替え */
export type MonitorMode = "stereo" | "mono" | "side" | "swap";

/** 聴く機器のシミュレーション: なし / スマホ / ノート PC / 前に置いたスピーカー(ヘッドホン用) */
export type SpeakerSim = "off" | "phone" | "laptop" | "front";

export type ChatEvent =
  | { kind: "started" }
  | { kind: "assistant_text"; text: string }
  | { kind: "tool_use"; name: string }
  | { kind: "result"; ok: boolean; text: string }
  | { kind: "notice"; text: string }
  | { kind: "error"; message: string };
