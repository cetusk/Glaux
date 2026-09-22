// project.json / MCP レスポンスに対応する型(glaux-core の serde 形と一致させる)

export type Articulation = "normal" | "palm_mute" | "staccato" | "accent";

export interface Note {
  id: string;
  pos: number;
  dur: number;
  pitch: number;
  vel: number;
  /** 奏法。省略 = normal */
  articulation?: Articulation;
}

export interface MidiClip {
  id: string;
  name: string;
  start: number;
  length: number;
  kind: "midi";
  notes: Note[];
  looped?: boolean;
}

export interface AudioClip {
  id: string;
  name: string;
  start: number;
  length: number;
  kind: "audio";
  asset: string;
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

export interface Track {
  id: string;
  name: string;
  kind: "midi" | "audio";
  color?: string;
  mute: boolean;
  solo: boolean;
  volume_db: number;
  pan: number;
  device?: { type?: string; name?: string; params?: Record<string, unknown> } | null;
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
  master: { volume_db: number; effects: unknown[] };
  assets: Record<string, unknown>;
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
}

export interface EffectView {
  id: string;
  name: string;
  bypass: boolean;
  params: ParamView[];
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
