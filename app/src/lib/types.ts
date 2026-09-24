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
  effects: unknown[];
  clips: Clip[];
  automation: AutomationLane[];
}

export interface Project {
  format: string;
  version: number;
  ppq: number;
  meta: { title: string; created: string };
  tempo_map: { tick: number; bpm: number }[];
  time_sig_map: { tick: number; num: number; den: number }[];
  tracks: Track[];
  master: { volume_db: number; effects: unknown[]; automation?: AutomationLane[] };
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
  entries: EntrySummary[];
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

export interface EffectView {
  id: string;
  /** 内蔵エフェクト名。CLAP プラグインは "clap" */
  name: string;
  bypass: boolean;
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
  dsp?: { avg_pct: number; max_pct: number; overruns: number; late: number; swaps: number };
  tick: number;
  /** ループ区間 [開始tick, 終了tick]。null ならループなし */
  loop?: [number, number] | null;
}

export type ChatEvent =
  | { kind: "started" }
  | { kind: "assistant_text"; text: string }
  | { kind: "tool_use"; name: string }
  | { kind: "result"; ok: boolean; text: string }
  | { kind: "notice"; text: string }
  | { kind: "error"; message: string };
