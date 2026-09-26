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
        outputDevice: typeof v.outputDevice === "string" ? v.outputDevice : "",
        inputDevice: typeof v.inputDevice === "string" ? v.inputDevice : "",
        bufferFrames: typeof v.bufferFrames === "number" ? v.bufferFrames : 0,
        autoGain: v.autoGain !== false,
        recordStereo: v.recordStereo === true,
        midiInput: typeof v.midiInput === "string" ? v.midiInput : "",
        midiQuantize: typeof v.midiQuantize === "number" ? v.midiQuantize : 0,
        welcomeDone: v.welcomeDone === true,
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
    outputDevice: "",
    inputDevice: "",
    bufferFrames: 0,
    autoGain: true,
    recordStereo: false,
    midiInput: "",
    midiQuantize: 0,
    welcomeDone: false,
  };
}

export const settings = $state<Settings>(load());

/// 設定画面の開閉と、開くページ(ステータスバーのデバイスから開くとオーディオのページなど)
export type SettingsTab = "display" | "audio" | "midi" | "record" | "ai";
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

/// モデルの選択肢("" は各 CLI の既定)。Claude は claude --model のエイリアス、GPT は codex -m のモデル名
export const CHAT_MODELS = {
  claude: [
    { value: "", label: "既定" },
    { value: "opus", label: "Opus" },
    { value: "sonnet", label: "Sonnet" },
    { value: "haiku", label: "Haiku" },
  ],
  codex: [
    { value: "", label: "既定" },
    { value: "gpt-6-astra", label: "GPT-6 Astra" },
  ],
};

export const CHAT_MODEL_EXAMPLES = {
  claude: "例: claude-opus-5-5、claude-sonnet-5",
  codex: "例: gpt-6-astra",
};

/** 今の相手のモデル名("" = 既定) */
export function currentChatModel(): string {
  return settings.chatProvider === "codex" ? settings.chatCodexModel : settings.chatModel;
}

export function setChatModel(v: string) {
  if (settings.chatProvider === "codex") settings.chatCodexModel = v;
  else settings.chatModel = v;
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
