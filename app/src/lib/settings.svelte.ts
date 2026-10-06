// DAW の設定(フロントエンドのみで完結するもの)。localStorage に保存。

export interface AccentPreset {
  name: string;
  label: string;
  accent: string;
  dim: string;
}

export const ACCENT_PRESETS: AccentPreset[] = [
  { name: "turquoise", label: "ターコイズ", accent: "#2dd4bf", dim: "#1f9e90" },
  { name: "amber", label: "アンバー", accent: "#ffc247", dim: "#b98c33" },
  { name: "violet", label: "バイオレット", accent: "#b07ce8", dim: "#8459b3" },
  { name: "sky", label: "スカイ", accent: "#38bdf8", dim: "#2a8fc0" },
  { name: "rose", label: "ローズ", accent: "#fb7185", dim: "#c05467" },
  { name: "lime", label: "ライム", accent: "#a3e635", dim: "#7cae28" },
];

const ACCENT_LABELS_EN: Record<string, string> = {
  turquoise: "Turquoise",
  amber: "Amber",
  violet: "Violet",
  sky: "Sky",
  rose: "Rose",
  lime: "Lime",
};

/** テーマカラーの表示名(今の表示の言語) */
export function accentLabel(p: AccentPreset): string {
  return t(p.label, ACCENT_LABELS_EN[p.name] ?? p.label);
}

interface Settings {
  accent: string;
  notifyOnAiDone: boolean;
  /// 録音: 出力レイテンシ補正(ms)。聴いて歌う分の遅れをこの値だけ前へ詰める
  recordLatencyMs: number;
  /// 録音: カウントインの小節数(0 で無し)
  countInBars: number;
  /// 録音中はメトロノームを自動で鳴らす
  metronomeOnRecord: boolean;
  /// チャットの相手。"claude" = Claude Code、"codex" = Codex CLI(GPT)
  chatProvider: "claude" | "codex";
  /// チャットの AI モデル(`claude --model` に渡す)。空文字 = Claude Code の既定
  chatModel: string;
  /// GPT のモデル(`codex -m` に渡す)。空文字 = Codex の既定
  chatCodexModel: string;
  /// 考える深さ(`claude --effort` / Codex の model_reasoning_effort)。空文字 = 既定
  chatEffort: string;
  chatCodexEffort: string;
  /// オーディオデバイス(空文字 = OS の既定)
  outputDevice: string;
  inputDevice: string;
  /// 出力バッファの大きさ(フレーム、0 = 既定の 1024)
  bufferFrames: number;
  /// 録音の音量を自動で整える(一番大きい所を -6dB に)
  autoGain: boolean;
  /// ステレオで録音する(入力が 2 ch 以上のとき。1 本のマイクならモノラルでよい)
  recordStereo: boolean;
  /// MIDI 入力ポート(空文字 = 使わない)
  midiInput: string;
  /// MIDI 録音の開始位置の丸め(tick、0 = 丸めない)
  midiQuantize: number;
  /// はじめの確認(音の出力・AI・SoundFont・デモ曲)を見た
  welcomeDone: boolean;
  /// 聴く音量(アプリから鳴る音だけ。曲・書き出しには入らない。dB)
  outputVolumeDb: number;
  /// 推定した計画を確認せずに採用する(設計画面の「次から確認せずに採用する」)
  autoAdoptEstimated: boolean;
  /// 作る前に AI が質問する: auto = 決め手が読み取れないとき / never = 途中で尋ねない(いつもおまかせ。最後まで AI が決める)
  chatAsk: "auto" | "never";
  /// 言語: 画面の表示と、AI の返答(チャットの返事・途中の一言・質問・AI が付ける名前)
  lang: "ja" | "en";
}

function load(): Settings {
  try {
    const raw = localStorage.getItem("glaux.settings");
    if (raw) {
      const v = JSON.parse(raw);
      return {
        accent: typeof v.accent === "string" ? v.accent : "turquoise",
        notifyOnAiDone: v.notifyOnAiDone !== false,
        recordLatencyMs: typeof v.recordLatencyMs === "number" ? v.recordLatencyMs : 60,
        countInBars: typeof v.countInBars === "number" ? v.countInBars : 1,
        metronomeOnRecord: v.metronomeOnRecord !== false,
        chatProvider: v.chatProvider === "codex" ? "codex" : "claude",
        chatModel: typeof v.chatModel === "string" ? v.chatModel : "",
        chatCodexModel: typeof v.chatCodexModel === "string" ? v.chatCodexModel : "",
        chatEffort: typeof v.chatEffort === "string" ? v.chatEffort : "",
        chatCodexEffort: typeof v.chatCodexEffort === "string" ? v.chatCodexEffort : "",
        outputDevice: typeof v.outputDevice === "string" ? v.outputDevice : "",
        inputDevice: typeof v.inputDevice === "string" ? v.inputDevice : "",
        bufferFrames: typeof v.bufferFrames === "number" ? v.bufferFrames : 0,
        autoGain: v.autoGain !== false,
        recordStereo: v.recordStereo === true,
        midiInput: typeof v.midiInput === "string" ? v.midiInput : "",
        midiQuantize: typeof v.midiQuantize === "number" ? v.midiQuantize : 0,
        welcomeDone: v.welcomeDone === true,
        outputVolumeDb: typeof v.outputVolumeDb === "number" ? v.outputVolumeDb : 0,
        autoAdoptEstimated: v.autoAdoptEstimated === true,
        chatAsk: v.chatAsk === "never" ? "never" : "auto",
        lang: (v.lang ?? v.chatLang) === "en" ? "en" : "ja",
      };
    }
  } catch {
    // 壊れていたら既定値
  }
  return {
    accent: "turquoise",
    notifyOnAiDone: true,
    recordLatencyMs: 60,
    countInBars: 1,
    metronomeOnRecord: true,
    chatProvider: "claude",
    chatModel: "",
    chatCodexModel: "",
    chatEffort: "",
    chatCodexEffort: "",
    outputDevice: "",
    inputDevice: "",
    bufferFrames: 0,
    autoGain: true,
    recordStereo: false,
    midiInput: "",
    midiQuantize: 0,
    welcomeDone: false,
    outputVolumeDb: 0,
    autoAdoptEstimated: false,
    chatAsk: "auto",
    lang: "ja",
  };
}

export const settings = $state<Settings>(load());

/// 表示の言語の対訳(i18n.svelte.ts の tr と同じ。i18n がこのモジュールを読むので、循環を避けてここにも持つ)
function t(ja: string, en: string): string {
  return settings.lang === "en" ? en : ja;
}

/// 設定画面の開閉と、開くページ(ステータスバーのデバイスから開くとオーディオのページなど)
export type SettingsTab = "display" | "audio" | "midi" | "record" | "ai" | "about";
export const settingsUi = $state<{ open: boolean; tab: SettingsTab }>({ open: false, tab: "display" });

/// はじめの確認の開閉(初回は自動で開く。設定の「表示」からいつでも開ける)
export const welcomeUi = $state<{ open: boolean }>({ open: !settings.welcomeDone });

export function openSettings(tab?: SettingsTab) {
  if (tab) settingsUi.tab = tab;
  settingsUi.open = true;
}

// ---- チャットの相手とモデル(チャットの見出しと設定の「AI」で共通) ----
// Claude = Claude Code(claude)、GPT = Codex CLI(codex)。どちらもこの PC でログイン済みのものを使う
export const CHAT_PROVIDERS = [
  { value: "claude", label: "Claude", cli: "Claude Code" },
  { value: "codex", label: "GPT", cli: "Codex CLI" },
] as const;

/// モデルの選択肢("" は各 CLI の既定)。名前を手で入れなくて済むよう、各 CLI が受け付けるものを並べる。
/// Claude は Claude Code 2.1.287 の正式名、GPT は Codex CLI 0.156.1 に組み込まれた一覧(画面に出すもの)。
/// efforts はそのモデルが受け付ける考える深さ(GPT はモデルごとに違う)
export interface ChatModelOption {
  value: string;
  label: string;
  note?: string;
  efforts?: readonly string[];
}

const CLAUDE_EFFORTS = ["low", "medium", "high", "xhigh", "max"] as const;
const GPT_EFFORTS = ["low", "medium", "high", "xhigh", "max", "ultra"] as const;
const GPT_EFFORTS_NO_ULTRA = ["low", "medium", "high", "xhigh", "max"] as const;

export const CHAT_MODELS: Record<"claude" | "codex", ChatModelOption[]> = {
  claude: [
    { value: "", label: "既定", note: "Claude Code の既定のモデル", efforts: CLAUDE_EFFORTS },
    { value: "claude-fable-5-1", label: "Fable 5.1", efforts: CLAUDE_EFFORTS },
    { value: "claude-opus-5-5", label: "Opus 5.5", efforts: CLAUDE_EFFORTS },
    { value: "claude-opus-5-5[1m]", label: "Opus 5.5(1M)", note: "長い文脈(100 万トークン)", efforts: CLAUDE_EFFORTS },
    { value: "claude-sonnet-5-5", label: "Sonnet 5.5", efforts: CLAUDE_EFFORTS },
    { value: "claude-sonnet-5-5[1m]", label: "Sonnet 5.5(1M)", note: "長い文脈(100 万トークン)", efforts: CLAUDE_EFFORTS },
    { value: "claude-haiku-4-5", label: "Haiku 4.5", note: "速くて軽い", efforts: CLAUDE_EFFORTS },
  ],
  codex: [
    { value: "", label: "既定", note: "Codex CLI の既定のモデル", efforts: GPT_EFFORTS },
    { value: "gpt-6-astra", label: "GPT-6 Astra", note: "最も高度な作業に", efforts: GPT_EFFORTS },
    { value: "gpt-6-sol", label: "GPT-6 Sol", note: "普段の作業に", efforts: GPT_EFFORTS },
    { value: "gpt-6-luna", label: "GPT-6 Luna", note: "速くて安い", efforts: GPT_EFFORTS_NO_ULTRA },
    { value: "gpt-5.6-sol", label: "GPT-5.6 Sol", note: "前の世代", efforts: GPT_EFFORTS },
    { value: "gpt-5.6-terra", label: "GPT-5.6 Terra", note: "前の世代", efforts: GPT_EFFORTS },
    { value: "gpt-5.6-luna", label: "GPT-5.6 Luna", note: "前の世代・速い", efforts: GPT_EFFORTS_NO_ULTRA },
    { value: "gpt-5.5", label: "GPT-5.5", note: "旧版", efforts: ["low", "medium", "high", "xhigh"] },
  ],
};

/// 考える深さの表示("" = 既定)
export const EFFORT_LABELS: Record<string, string> = {
  "": "既定",
  minimal: "最小",
  low: "low(軽く速く)",
  medium: "medium(標準)",
  high: "high(じっくり)",
  xhigh: "xhigh(さらにじっくり)",
  max: "max(最大)",
  ultra: "ultra(最も深く・遅い)",
};

// 表の表示名は日本語のまま(値・照合にも使う)。英語の表示は使う所で下の関数を通す
const MODEL_TEXT_EN: Record<string, string> = {
  既定: "Default",
  "Claude Code の既定のモデル": "Claude Code's default model",
  "Codex CLI の既定のモデル": "Codex CLI's default model",
  "長い文脈(100 万トークン)": "Long context (1M tokens)",
  速くて軽い: "Fast and light",
  最も高度な作業に: "For the most advanced work",
  普段の作業に: "For everyday work",
  速くて安い: "Fast and cheap",
  前の世代: "Previous generation",
  "前の世代・速い": "Previous generation, fast",
  旧版: "Legacy",
};

const EFFORT_LABELS_EN: Record<string, string> = {
  "": "Default",
  minimal: "minimal",
  low: "low (light, fast)",
  medium: "medium (standard)",
  high: "high (careful)",
  xhigh: "xhigh (more careful)",
  max: "max (maximum)",
  ultra: "ultra (deepest, slow)",
};

/** モデルの名前・説明(CHAT_MODELS の label / note)の表示。英語では日本語の所だけ訳す(モデル名はそのまま) */
export function chatModelText(text: string): string {
  return t(text, MODEL_TEXT_EN[text] ?? text.replace("(1M)", " (1M)"));
}

/** 考える深さの表示名(今の表示の言語) */
export function effortLabel(ef: string): string {
  return t(EFFORT_LABELS[ef] ?? ef, EFFORT_LABELS_EN[ef] ?? ef);
}

/** 今の相手のモデル名("" = 既定) */
export function currentChatModel(): string {
  return settings.chatProvider === "codex" ? settings.chatCodexModel : settings.chatModel;
}

/** 今の相手の考える深さ("" = 既定) */
export function currentChatEffort(): string {
  return settings.chatProvider === "codex" ? settings.chatCodexEffort : settings.chatEffort;
}

/** モデルが受け付ける考える深さ(一覧に無いモデル = 以前に手で入れた名前は、その相手の既定のモデルと同じ) */
export function effortsFor(provider: "claude" | "codex", model: string): readonly string[] {
  const list = CHAT_MODELS[provider];
  return (list.find((m) => m.value === model) ?? list[0]).efforts ?? [];
}

export function setChatModel(v: string) {
  if (settings.chatProvider === "codex") settings.chatCodexModel = v;
  else settings.chatModel = v;
  // 新しいモデルが受け付けない深さは既定に戻す
  if (!effortsFor(settings.chatProvider, v).includes(currentChatEffort())) setChatEffort("");
  saveSettings();
}

export function setChatEffort(v: string) {
  if (settings.chatProvider === "codex") settings.chatCodexEffort = v;
  else settings.chatEffort = v;
  saveSettings();
}

export function saveSettings() {
  try {
    localStorage.setItem("glaux.settings", JSON.stringify({ ...settings }));
  } catch {
    // localStorage が使えなくても動作に支障なし
  }
}

/** アクセントカラーを CSS 変数に反映する(起動時と変更時に呼ぶ)。 */
export function applyTheme() {
  const preset =
    ACCENT_PRESETS.find((p) => p.name === settings.accent) ?? ACCENT_PRESETS[0];
  const root = document.documentElement;
  root.style.setProperty("--accent", preset.accent);
  root.style.setProperty("--accent-dim", preset.dim);
}

// ---- AI 完了通知(WebAudio の短いチャイム。プラグイン不要) ----

let audioCtx: AudioContext | undefined;

function beep(freq: number, start: number, dur: number, gain: number) {
  if (!audioCtx) return;
  const t0 = audioCtx.currentTime + start;
  const osc = audioCtx.createOscillator();
  const g = audioCtx.createGain();
  osc.type = "triangle";
  osc.frequency.value = freq;
  g.gain.setValueAtTime(0, t0);
  g.gain.linearRampToValueAtTime(gain, t0 + 0.01);
  g.gain.exponentialRampToValueAtTime(0.001, t0 + dur);
  osc.connect(g).connect(audioCtx.destination);
  osc.start(t0);
  osc.stop(t0 + dur + 0.05);
}

/** AI のターン完了チャイム(成功)。 */
export function playDoneChime() {
  if (!settings.notifyOnAiDone) return;
  try {
    audioCtx ??= new AudioContext();
    beep(880, 0, 0.18, 0.12);
    beep(1318.5, 0.12, 0.28, 0.1); // E6: 明るい上行
  } catch {
    // 音が出せない環境では黙って無視
  }
}

/** AI のターン失敗の通知(低い音)。 */
export function playErrorChime() {
  if (!settings.notifyOnAiDone) return;
  try {
    audioCtx ??= new AudioContext();
    beep(220, 0, 0.35, 0.12);
  } catch {
    // 同上
  }
}
