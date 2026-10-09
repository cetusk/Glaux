// 音色エディタ: ウェーブテーブル。テーブルは「作り方 → 手で編集した波形 → 加工の手順」で作る。
// 手順は JSON(テーブルの素材の隣の .recipe.json)に残り、いつでも直せる(AI も make_wavetable で同じ形を書く)。
// 絵の材料(重ねた波形・選んだ 1 枚の加工の前と後・倍音)はエンジンで作って受け取る(加工の式をエンジンに合わせる)。
// 手で描いた形は画面側で倍音に分け(256 本まで)、手順の keys としてエンジンへ渡す
import { save as pickSave } from "@tauri-apps/plugin-dialog";
import * as api from "../api";
import { tr } from "../i18n.svelte";
import { showToast } from "../toast.svelte";
import { draw as drawAll, register, setSel, toast, type Data, type Loaded } from "./core.svelte";
import { axisRow, byId, clamp, css, ctx2d, el, hzRange, ico, midiHz, noteSpan, stripe } from "./dom";
import { canEdit, cur, knob, legendRow, pitchPick, pushHist, rangeOf, rerender, shown, specBox, state, ui } from "./parts";
import { listen } from "./core.svelte";

/** 倍音を描く欄の棒の数 */
const HB = 96;
/** 手で編集した波形の倍音の数(絵の倍音と鳴らす段の計算もこの数まで) */
const HK = 256;
/** 1 周期の点の数(エンジンから受け取る・手で描く) */
const N = 512;
const PTS = 16;
const TABLE_NAMES = ["analog", "pulse", "vocal", "sync", "organ", "fm", "growl", "fold", "harmonic", "digital"];

const SHAPES = (): [string, string, string][] => [
  ["sine_to_saw", tr("正弦 → ノコギリ", "Sine → saw"), tr("倍音が下から順に増える。丸い → 明るい", "Harmonics fill in from the bottom; round → bright")],
  ["sine_to_square", tr("正弦 → 矩形", "Sine → square"), tr("奇数倍音が増える。丸い → 中空の太い音", "Odd harmonics grow; round → hollow and thick")],
  ["analog", tr("アナログ", "Analog"), tr("正弦 → 三角 → ノコギリ → 矩形", "Sine → triangle → saw → square")],
  ["pwm", tr("パルス幅", "Pulse width"), tr("50% → 5%(細く鼻にかかる)", "50% → 5% (thin, nasal)")],
  ["sync", tr("ハードシンク", "Hard sync"), tr("1 → 8 倍(ギラついた金属的な変化)", "1 → 8× (glaring, metallic)")],
  ["fm", "FM", tr("変調の深さ 0 → 深い(ベル・金属)", "Mod depth 0 → deep (bell, metal)")],
  ["fm_octave", tr("FM(1 オクターブ上)", "FM (octave up)"), tr("比 2 で変調(明るく澄んだ倍音)", "Modulated at ratio 2 (bright, clear)")],
  ["vowels", tr("母音", "Vowels"), tr("あ → え → い → お → う", "a → e → i → o → u")],
  ["growl", tr("グロウル", "Growl"), tr("うなる中域(ダブステップのベース)", "Growling mids (dubstep bass)")],
  ["fold", tr("折り返し", "Fold"), tr("正弦を折り返す(1 → 8 回)", "Folds a sine (1 → 8×)")],
  ["harmonic_sweep", tr("倍音の掃引", "Harmonic sweep"), tr("明るい倍音の帯が上へ動く", "A bright band of harmonics moves up")],
  ["digital", tr("デジタル", "Digital"), tr("段のある波形(ビットの粗さ)", "Stepped wave (bit-crushed)")],
  ["organ", tr("オルガン", "Organ"), tr("ドローバーを順に足す", "Drawbars added one by one")],
];
type OpDef = { name: string; desc: string; params: [string, string, number, number, number, number][] };
const OPS = (): Record<string, OpDef> => ({
  tilt: { name: tr("明るさの傾き", "Tilt"), desc: tr("高域を上げ下げ(オクターブあたり dB)", "Raise or lower the highs (dB per octave)"), params: [["db", "dB/oct", -12, 12, 0.5, -2]] },
  oddeven: { name: tr("奇数・偶数", "Odd / even"), desc: tr("奇数・偶数の倍音の量", "Amount of odd and even harmonics"), params: [["odd", "奇数", 0, 2, 0.05, 1], ["even", "偶数", 0, 2, 0.05, 0.4]] },
  band: { name: tr("倍音の帯", "Harmonic band"), desc: tr("何番目〜何番目を上げ下げ(−100 dB で消す)", "Raise or lower harmonics N–M (−100 dB removes them)"), params: [["from", "から", 1, HB, 1, 6], ["to", "まで", 1, HB, 1, 14], ["db", "dB", -100, 12, 1, 6]] },
  lowpass: { name: tr("ローパス", "Low-pass"), desc: tr("倍音をここまでに絞る", "Keep harmonics up to here"), params: [["max", "番目まで", 1, HB, 1, 24]] },
  saturate: { name: tr("飽和", "Saturate"), desc: tr("丸めて倍音を足す(太く)", "Rounds off and adds harmonics (thicker)"), params: [["drive", "強さ", 1, 20, 0.1, 3]] },
  fold: { name: tr("折り返し", "Fold"), desc: tr("折り返して荒らす", "Folds over for grit"), params: [["gain", "強さ", 1, 8, 0.1, 1.5]] },
  phase: { name: tr("位相", "Phase"), desc: tr("そろえる / 波形どうしでそろえる / ばらす", "Zero / align between frames / randomize"), params: [["mode", "そろえ方", 0, 2, 1, 1]] },
  smooth: { name: tr("なめらかに", "Smooth"), desc: tr("隣の波形となじませる(位置の変化がなめらか)", "Blend with neighboring frames (smoother position changes)"), params: [["amount", "量", 0, 1, 0.05, 0.4]] },
  reverse: { name: tr("逆順", "Reverse"), desc: tr("波形の並びを逆に", "Reverse the frame order"), params: [] },
  select: { name: tr("切り出し", "Select"), desc: tr("位置の一部だけを使う", "Use only part of the positions"), params: [["from", "から", 0, 1, 0.01, 0.2], ["to", "まで", 0, 1, 0.01, 0.8]] },
  removedc: { name: tr("直流を除く", "Remove DC"), desc: tr("波形の上下のずれをなくす(録った音から作ったとき)", "Removes the up/down offset (for recorded sources)"), params: [] },
  normalize: { name: tr("正規化", "Normalize"), desc: tr("波形ごとに音量をそろえる", "Even out each frame's level"), params: [] },
  formant: { name: tr("フォルマントずらし", "Formant shift"), desc: tr("倍音の山だけを上下(声・胴の響き。音の高さは変わらない)", "Move only the harmonic hills up or down (voice, body resonance; pitch stays)"), params: [["st", "半音", -24, 24, 0.5, 5]] },
  blur: { name: tr("倍音のぼかし", "Spectral blur"), desc: tr("隣の倍音となじませて、くっきりした山・谷を丸める", "Blend neighboring harmonics to soften sharp peaks and dips"), params: [["amount", "量", 0, 1, 0.05, 0.3]] },
  pd: { name: tr("位相歪み", "Phase distortion"), desc: tr("1 周期の読み方を曲げる(フィルタを開くような明るさ)", "Bends how the cycle is read (bright, like opening a filter)"), params: [["amount", "強さ", 0, 1, 0.05, 0.5]] },
  sync: { name: tr("ハードシンク", "Hard sync"), desc: tr("1 周期の中で速く読み直す(ギラついた倍音)", "Re-reads faster within the cycle (edgy harmonics)"), params: [["ratio", "倍率", 1, 8, 0.1, 2]] },
});

/** 加工のアイコン(足した加工は近いアイコンを借りる) */
const opIcon = (k: string) => ({ formant: "fpos", blur: "smooth", pd: "phase", sync: "loop" })[k as "formant"] ?? k;

// ---- 作り方の手順(エンジンの make_wavetable と同じ形) ----
const r5 = (v: number) => Math.round(v * 1e5) / 1e5;
function sourceOf(src: Data): Data {
  switch (src.kind) {
    case "shape":
      return { kind: "shape", name: src.name };
    case "harmonics":
      return { kind: "harmonics" };
    case "audio":
      return { kind: "audio_asset", id: src.id, from: 0, to: src.range ?? 1 };
    default:
      return { ...src };
  }
}
function editOf(op: Data): Data {
  const p = op.p ?? {};
  // AI が付けた位置ごとの効き方(fade)は画面で開き直しても残す
  const meta = { on: op.on, who: op.who, id: op.id, ...(op.fade ? { fade: op.fade } : {}) };
  switch (op.type) {
    case "tilt":
      return { op: "tilt", db_per_octave: p.db, ...meta };
    case "oddeven":
      return { op: "odd_even", odd: p.odd, even: p.even, ...meta };
    case "band":
      return { op: "band", from: Math.round(p.from), to: Math.round(p.to), gain_db: p.db, ...meta };
    case "lowpass":
      return { op: "lowpass", max: Math.round(p.max), ...meta };
    case "saturate":
      return { op: "saturate", drive: p.drive, ...meta };
    case "fold":
      return { op: "fold", gain: p.gain, ...meta };
    case "phase":
      return { op: "phase", mode: ["zero", "align", "random"][clamp(Math.round(p.mode), 0, 2)], seed: 1, ...meta };
    case "smooth":
      return { op: "smooth", amount: p.amount, ...meta };
    case "reverse":
      return { op: "reverse", ...meta };
    case "select":
      return { op: "select", from: p.from, to: p.to, ...meta };
    case "removedc":
      return { op: "remove_dc", ...meta };
    case "normalize":
      return { op: "normalize", per_frame: true, ...meta };
    case "formant":
      return { op: "formant_shift", semitones: p.st, ...meta };
    case "blur":
      return { op: "spectral_blur", amount: p.amount, ...meta };
    case "pd":
      return { op: "phase_distort", amount: p.amount, ...meta };
    case "sync":
      return { op: "sync", ratio: p.ratio, ...meta };
    default:
      return { ...(op.raw ?? {}), ...meta };
  }
}
/** 手順の加工 → 画面の加工(知らない加工は名前だけのカードにする) */
function opOf(e: Data, i: number): Data {
  const base = { id: e.id ?? i + 1, on: e.on !== false, who: e.who ?? "ai", ...(e.fade ? { fade: e.fade } : {}) };
  switch (e.op) {
    case "tilt":
      return { ...base, type: "tilt", p: { db: e.db_per_octave ?? 0 } };
    case "odd_even":
      return { ...base, type: "oddeven", p: { odd: e.odd ?? 1, even: e.even ?? 1 } };
    case "band":
      return { ...base, type: "band", p: { from: e.from ?? 1, to: e.to ?? 1, db: e.gain_db ?? 0 } };
    case "lowpass":
      return { ...base, type: "lowpass", p: { max: e.max ?? 24 } };
    case "saturate":
      return { ...base, type: "saturate", p: { drive: e.drive ?? 3 } };
    case "fold":
      return { ...base, type: "fold", p: { gain: e.gain ?? 1.5 } };
    case "phase":
      return { ...base, type: "phase", p: { mode: Math.max(0, ["zero", "align", "random"].indexOf(e.mode)) } };
    case "smooth":
      return { ...base, type: "smooth", p: { amount: e.amount ?? 0.4 } };
    case "reverse":
      return { ...base, type: "reverse", p: {} };
    case "select":
      return { ...base, type: "select", p: { from: e.from ?? 0, to: e.to ?? 1 } };
    case "remove_dc":
      return { ...base, type: "removedc", p: {} };
    case "normalize":
      return { ...base, type: "normalize", p: {} };
    case "formant_shift":
      return { ...base, type: "formant", p: { st: e.semitones ?? 0 } };
    case "spectral_blur":
      return { ...base, type: "blur", p: { amount: e.amount ?? 0.3 } };
    case "phase_distort":
      return { ...base, type: "pd", p: { amount: e.amount ?? 0.5 } };
    case "sync":
      return { ...base, type: "sync", p: { ratio: e.ratio ?? 2 } };
    default: {
      const raw = { ...e };
      delete raw.on;
      delete raw.who;
      delete raw.id;
      return { ...base, type: "raw", raw, p: {} };
    }
  }
}
export function recipeOf(d: Data): Data {
  return {
    source: sourceOf(d.src),
    frames: d.frames,
    keys: d.keys.map((k: Data) => ({
      pos: r5(k.pos),
      amps: k.amps.map(r5),
      ...(k.ph ? { phases: k.ph.map((v: number) => r5(v / (2 * Math.PI))) } : {}),
      who: k.who,
      ...(k.pts ? { pts: k.pts.map(r5) } : {}),
    })),
    blend: d.blend,
    edits: d.ops.map(editOf),
  };
}

/** 1 周期の波形(N 点)から倍音(強さ・位相〈ラジアン〉)を求める */
function dft(x: ArrayLike<number>): { amps: number[]; ph: number[] } {
  const n = x.length;
  const amps: number[] = new Array(HK).fill(0),
    ph: number[] = new Array(HK).fill(0);
  for (let k = 1; k <= HK; k++) {
    let re = 0,
      im = 0;
    for (let i = 0; i < n; i++) {
      const a = (2 * Math.PI * k * i) / n;
      re += x[i] * Math.cos(a);
      im += x[i] * Math.sin(a);
    }
    amps[k - 1] = (2 / n) * Math.hypot(re, im);
    ph[k - 1] = Math.atan2(re, im);
  }
  return { amps, ph };
}
/** 倍音(1 番目から)→ 1 周期(N 点)。kmax 本より上は鳴らさない */
function idft(amps: ArrayLike<number>, ph: ArrayLike<number> | null, kmax = HK): Float32Array {
  const x = new Float32Array(N);
  const K = Math.min(amps.length, kmax);
  for (let k = 1; k <= K; k++) {
    const a = amps[k - 1];
    if (a < 1e-6) continue;
    const p = ph ? ph[k - 1] : 0;
    for (let n = 0; n < N; n++) x[n] += a * Math.sin((2 * Math.PI * k * n) / N + p);
  }
  return x;
}

/** 編集した波形が位置 t の波形にどれだけ効くか(0〜1)。その位置にいちばん近い波形は 1、なじませる幅の外は 0 */
function keyWeight(k: Data, t: number, F: number, blend: number) {
  const dist = Math.abs(t - k.pos);
  if (dist <= 0.5 / Math.max(1, F - 1)) return 1;
  if (dist >= blend) return 0;
  return 0.5 * (1 + Math.cos((Math.PI * dist) / blend));
}
/** 位置 t の波形がどう作られたか: "ed"(編集した)/ "auto"(自動)/ "src"(作り方のまま) */
function frameKind(d: Data, t: number, F: number) {
  if (d.keys.some((k: Data) => Math.abs(t - k.pos) <= 0.5 / Math.max(1, F - 1))) return "ed";
  if (!d.keys.length) return "src";
  if (d.src.kind === "harmonics") return "auto";
  return d.keys.some((k: Data) => keyWeight(k, t, F, d.blend ?? 0.15) > 0) ? "auto" : "src";
}
/** 加工の手順のうち位置を入れ替える物(逆順・切り出し)をたどって、表(出口)の位置 → 作り方(加工の前)の位置に読み替える */
function wtSrcPos(d: Data, p: number) {
  const ops = d.ops.filter((o: Data) => o.on && (o.type === "reverse" || o.type === "select"));
  for (let i = ops.length - 1; i >= 0; i--) {
    const o = ops[i];
    if (o.type === "reverse") p = 1 - p;
    else {
      const a = Math.min(o.p.from, o.p.to),
        b = Math.max(o.p.from, o.p.to);
      p = a + (b - a) * p;
    }
  }
  return clamp(p, 0, 1);
}
/** 作り方の位置 → 表(出口)の位置。切り出しの外なら null */
function wtOutPos(d: Data, p: number): number | null {
  for (const o of d.ops.filter((o: Data) => o.on && (o.type === "reverse" || o.type === "select"))) {
    if (o.type === "reverse") p = 1 - p;
    else {
      const a = Math.min(o.p.from, o.p.to),
        b = Math.max(o.p.from, o.p.to);
      if (b - a < 1e-6 || p < a - 1e-6 || p > b + 1e-6) return null;
      p = (p - a) / (b - a);
    }
  }
  return p;
}
/** 位置 → 波形の番号(何枚目か)。編集した波形かどうかは番号で比べる */
const wtIdx = (d: Data, p: number) => Math.round(clamp(p, 0, 1) * Math.max(1, d.frames - 1));
/** 表(出口)の位置 p の波形を作っている編集した波形(作り方の位置で持っている) */
function wtKeyAt(d: Data, p: number) {
  const i = wtIdx(d, wtSrcPos(d, p));
  return d.keys.find((k: Data) => wtIdx(d, k.pos) === i);
}
/** その鍵盤で鳴る倍音の数。エンジン(wavetable.rs の level_for)と同じく、倍音を半分ずつ減らした段(1023・511・255 … 本)を
 *  高さで切り替える(48 kHz で 0.45 × 標本化周波数 ÷ 周波数 に収まる最も豊かな段)。絵はこの画面が受け取る 256 本まで */
const audibleHarmonics = (midi: number) => {
  const allowed = Math.max(1, Math.floor((0.45 * 48000) / midiHz(midi)));
  let lv = 0;
  while (lv + 1 < 11 && 1023 >> lv > allowed) lv++;
  return clamp(1023 >> lv, 1, HK);
};
/** 段の色(低い段から): 波形の線・凡例・倍音の「ここまで」の線で同じ色 */
const WT_STEP_COLORS = ["#25bdb1", "#8fd16a", "#e8c46a", "#e88a4a", "#d4508a", "#9a7ae8"];
const wtStepColor = (i: number) => WT_STEP_COLORS[Math.min(i, WT_STEP_COLORS.length - 1)];
/** 音域の中の段: 同じ倍音の数で鳴る鍵盤のまとまり(低い方から) */
const wtSteps = (lo: number, hi: number) => {
  const out: { from: number; to: number; k: number }[] = [];
  for (let m = lo; m <= hi; m++) {
    const k = audibleHarmonics(m);
    const last = out[out.length - 1];
    if (last && last.k === k) last.to = m;
    else out.push({ from: m, to: m, k });
  }
  return out;
};
/** 周期の 16 点を通るなめらかな形(周期的な Catmull-Rom) */
function splineWave(pts: number[]) {
  const x = new Float32Array(N),
    P = pts.length;
  for (let n = 0; n < N; n++) {
    const u = (n / N) * P,
      i = Math.floor(u),
      t = u - i;
    const p0 = pts[(i - 1 + P) % P],
      p1 = pts[i % P],
      p2 = pts[(i + 1) % P],
      p3 = pts[(i + 2) % P];
    x[n] = clamp(0.5 * (2 * p1 + (-p0 + p2) * t + (2 * p0 - 5 * p1 + 4 * p2 - p3) * t * t + (-p0 + 3 * p1 - 3 * p2 + p3) * t * t * t), -1.2, 1.2);
  }
  return x;
}
/** 編集した波形の印の帯の中の横の位置(両端 10px は余白) */
const keyLeft = (pos: number) => `calc(10px + ${pos} * (100% - 20px))`;

// ---- エンジンから受け取る絵の材料 ----
let viewVal: api.WtView | null = null;
let viewKey = "";
let viewWant = "";
let viewBusy = false;
/** 左の列の小さな絵(元ごと) */
const thumbs = new Map<string, number[][] | "loading" | null>();
let shelfNames: { name: string; frames?: number }[] | null = null;

const WT: any = {
  kind: "wavetable",
  title: () => tr("ウェーブテーブル", "Wavetable"),
  async load({ params, project }: Loaded): Promise<Data> {
    const table = typeof params.table === "string" && params.table ? params.table : "analog";
    const base: Data = { pos: typeof params.position === "number" ? params.position : 0.3, lfo: typeof params.lfo_depth === "number" ? params.lfo_depth : 0, lfoRate: typeof params.lfo_rate === "number" ? params.lfo_rate : 2, tableId: table };
    if (TABLE_NAMES.includes(table)) return { ...base, src: { kind: "builtin", name: table }, frames: 16, keys: [], blend: 0.15, ops: [] };
    const r = await api.wavetableRecipe(table).catch(() => null);
    if (!r || !r.source) {
      const a = (project.assets as any)?.[table];
      const frames = a ? Math.max(2, Math.round(a.frames / 2048)) : 64;
      return { ...base, src: { kind: "asset", id: table }, frames, keys: [], blend: 0.15, ops: [] };
    }
    const s = r.source;
    const src: Data =
      s.kind === "shape"
        ? { kind: "shape", name: s.name }
        : s.kind === "harmonics"
          ? { kind: "harmonics" }
          : s.kind === "audio_asset"
            ? { kind: "audio", id: s.id, range: s.to ?? 1 }
            : { ...s };
    // 倍音の設計図を source の中に持つ手順(AI が make_wavetable で書いた物)は、その点を編集した波形として見せる
    const rawKeys: Data[] = Array.isArray(r.keys) && r.keys.length ? r.keys : s.kind === "harmonics" && Array.isArray(s.keys) ? s.keys : [];
    const keys = rawKeys.map((k) => ({
      pos: k.pos,
      amps: Array.from({ length: HK }, (_, i) => k.amps?.[i] ?? 0),
      ph: k.phases ? Array.from({ length: HK }, (_, i) => (k.phases[i] ?? 0) * 2 * Math.PI) : null,
      pts: k.pts ?? null,
      who: k.who ?? "ai",
    }));
    return { ...base, src, frames: r.frames ?? 64, keys, blend: typeof r.blend === "number" ? r.blend : 0.15, ops: (r.edits ?? []).map(opOf) };
  },
  uiInit: (): Data => ({ tool: "view", selOp: null, selKey: null, previewNote: 60 }),
  params: (d: Data) => ({ position: d.pos, lfo_depth: d.lfo }),
  signature: (d: Data) => JSON.stringify(recipeOf(d)),
  /** 手順が変わったら、テーブルを作って table を差し替える(つまみの差 cmds も同じ 1 件に) */
  async commit(cmds: unknown[], label: string, next: Data) {
    const r = await api.wavetableCommit(state.track, recipeOf(next), label, cmds);
    return { entry_id: r.entry_id, patch: { tableId: r.asset_id } };
  },
  /** 手順を変えている間の試聴: 仮のテーブルを当てる */
  previewExtra: (next: Data) => api.wavetablePreview(state.track, recipeOf(next)),
  staticParams: { lfo_depth: 0, pos_env: 0, mod1_position: 0, mod2_position: 0 },
  showShelf: false,
  lfoT: 0,
  hoverK: null as number | null,
  hoverPt: null as number | null,
  hiStep: null as number | null,
  drawnLive: null as Float32Array | null,
  reload() {
    viewKey = "";
    viewVal = null;
  },
  /** 絵の材料(無ければ取りに行き、届いたら描き直す。届くまでは前の材料) */
  view(d: Data): api.WtView | null {
    const sp = wtSrcPos(d, d.pos);
    const key = JSON.stringify([recipeOf(d), d.pos, sp]);
    if (key === viewKey) return viewVal;
    viewWant = key;
    if (!viewBusy) {
      viewBusy = true;
      const want = key,
        recipe = recipeOf(d),
        pos = d.pos;
      api
        .wavetableView(state.track, recipe, pos, sp)
        .then((v) => {
          viewVal = v;
          viewKey = want;
        })
        .catch(() => {
          viewKey = want;
        })
        .finally(() => {
          viewBusy = false;
          if (state.inst === "wavetable") WT.draw();
        });
    }
    return viewVal;
  },
  /** 加工の前の、選んでいる位置の波形(手で描くのはここ) */
  pre(d: Data): { x: Float32Array; amps: number[]; ph: number[] } | null {
    const v = WT.view(d);
    if (!v) return null;
    return { x: Float32Array.from(v.pre.wave), amps: v.pre.amps, ph: v.pre.phases };
  },
  thumb(src: Data): number[][] | null {
    const key = JSON.stringify(src);
    const t = thumbs.get(key);
    if (t === "loading") return null;
    if (t !== undefined) return t;
    thumbs.set(key, "loading");
    api
      .wavetableThumbs(state.track, [sourceOf(src)])
      .then((r) => {
        thumbs.set(key, r[0]);
        if (state.inst === "wavetable") rerender();
      })
      .catch(() => thumbs.set(key, null));
    return null;
  },
  /** 素材の候補(曲の中の音声クリップ) */
  audioSources(): { id: string; name: string; secs: number }[] {
    const p = state.project;
    if (!p) return [];
    const out = new Map<string, { id: string; name: string; secs: number }>();
    for (const t of p.tracks)
      for (const c of t.clips as any[]) {
        if (c.kind !== "audio" || !c.asset || out.has(c.asset)) continue;
        const a = (p.assets as any)?.[c.asset];
        out.set(c.asset, { id: c.asset, name: c.name || t.name, secs: a ? a.frames / Math.max(1, a.sample_rate) : 0 });
      }
    return [...out.values()];
  },
  render(body: HTMLElement) {
    const d = cur();
    body.innerHTML = "";
    const setSrc = (src: Data, label: string) => {
      if (!canEdit()) return;
      // 編集した波形は残す(新しい作り方の上に、同じ位置で差し替わる)
      d.src = src;
      WT.showShelf = false;
      pushHist(label);
      rerender();
    };
    // ---- 左: 作り方 ----
    const left = el("div", { class: "col left" });
    const srcSeg = el("div", { class: "seg", style: "flex-wrap:wrap" });
    const kinds: [string, string][] = [
      ["shape", tr("よくある形", "Common shapes")],
      ["harmonics", tr("倍音から", "From harmonics")],
      ["audio", tr("録った音から", "From recorded audio")],
      ["shelf", tr("保存した波形", "Saved tables")],
    ];
    kinds.forEach(([k, n]) =>
      srcSeg.append(
        el(
          "button",
          {
            class: (k === "shelf" ? WT.showShelf : d.src.kind === k && !WT.showShelf) ? "on" : "",
            onclick: () => {
              WT.showShelf = k === "shelf";
              if (k === "harmonics" && d.src.kind !== "harmonics") setSrc({ kind: "harmonics" }, tr("倍音から作る", "Build from harmonics"));
              else if (k !== "shelf" && k !== d.src.kind) {
                if (k === "audio") {
                  const a = WT.audioSources()[0];
                  if (a) setSrc({ kind: "audio", id: a.id, range: 1 }, tr(`録った音から作る: ${a.name}`, `Build from recorded audio: ${a.name}`));
                  else {
                    WT.showAudio = true;
                    rerender();
                  }
                } else setSrc({ kind: "shape", name: "sine_to_saw" }, tr("よくある形から作る", "Build from a common shape"));
              } else rerender();
            },
          },
          n,
        ),
      ),
    );
    const showAudio = d.src.kind === "audio" || (WT.showAudio && !WT.showShelf);
    WT.showAudio = false;
    const srcBox = el("div", { class: "box" }, el("h3", {}, tr("作り方", "How it's made"), el("span", { class: "spacer" }), el("span", { class: "hint" }, tr("どこから作るか", "Where it starts from"))), srcSeg);
    const thumbCanvas = (src: Data, audio = false) => {
      const c = el("canvas");
      requestAnimationFrame(() => {
        const th = WT.thumb(src);
        if (!th || !c.isConnected) return;
        const { g, w, h } = ctx2d(c);
        g.strokeStyle = "#7fd3cc";
        g.lineWidth = 1;
        const parts = audio ? 1 : th.length;
        for (let j = 0; j < parts; j++) {
          const x = th[audio ? 1 : j];
          g.beginPath();
          x.forEach((v: number, n: number) => {
            const xx = (j + n / x.length) * (w / parts),
              y = h / 2 - v * (h / 2 - 2) * 0.8;
            if (n) g.lineTo(xx, y);
            else g.moveTo(xx, y);
          });
          g.stroke();
        }
      });
      return c;
    };
    if (d.src.kind === "builtin" || d.src.kind === "asset" || d.src.kind === "library") {
      srcBox.append(
        el(
          "div",
          { class: "hint" },
          d.src.kind === "builtin"
            ? tr(`今のテーブル: 内蔵の「${d.src.name}」。下から選ぶか、真ん中で調整すると、このトラック用のテーブルになる`, `Current table: built-in "${d.src.name}". Pick one below or edit in the middle to make a table for this track`)
            : d.src.kind === "library"
              ? tr(`今のテーブル: 保存した波形「${d.src.name}」`, `Current table: saved table "${d.src.name}"`)
              : tr("今のテーブル: 読み込んだテーブル(作り方の手順なし)", "Current table: an imported table (no recipe)"),
        ),
      );
    }
    const shapes = el("div", { class: "shapes" });
    SHAPES().forEach(([k, n, desc]) => {
      shapes.append(
        el(
          "div",
          { class: "shape" + (d.src.kind === "shape" && d.src.name === k ? " on" : ""), title: desc, onclick: () => setSrc({ kind: "shape", name: k }, tr(`形を「${n}」に`, `Shape → "${n}"`)) },
          thumbCanvas({ kind: "shape", name: k }),
          el("div", {}, n, el("small", {}, desc)),
        ),
      );
    });
    const panel = (on: boolean, ...kids: (Node | string | null)[]) => el("div", { class: "srcpanel" + (on ? " on" : "") }, ...kids);
    const shapePanel = panel(d.src.kind !== "harmonics" && !showAudio && !WT.showShelf, el("div", { class: "hint" }, tr("左 → 右へ、0% → 100% の変わり方", "Left → right: how it changes from 0% to 100%")), shapes);
    const harmPanel = panel(
      d.src.kind === "harmonics" && !WT.showShelf,
      el(
        "div",
        { class: "hint" },
        tr(
          "サイン波(1 本目の倍音だけ)から始めます。真ん中の上で調整する波形を選び、下の枠の「点をつまむ」「線で描く」(波形)や「倍音を描く」で、いくつかの位置の波形を手で決めます。その間の波形は、前後を少しずつ混ぜて自動で作ります。",
          "Starts from a sine (just the 1st harmonic). Pick a frame above in the middle, then shape a few positions by hand with “Drag points”, “Draw” (waveform) or “Draw harmonics”. Frames in between are made automatically by blending their neighbors.",
        ),
      ),
      el("div", { class: "hint" }, tr(`編集した波形: ${d.keys.length} か所`, `Edited frames: ${d.keys.length}`)),
    );
    const clips = el("div", { class: "cliplist" });
    const auds = WT.audioSources();
    if (!auds.length) clips.append(el("div", { class: "hint" }, tr("曲の中に音声のクリップがありません", "There are no audio clips in the song")));
    auds.forEach((a: any) =>
      clips.append(
        el(
          "div",
          { class: "clipitem" + (d.src.kind === "audio" && d.src.id === a.id ? " on" : ""), onclick: () => setSrc({ kind: "audio", id: a.id, range: d.src.kind === "audio" ? (d.src.range ?? 1) : 1 }, tr(`録った音から作る: ${a.name}`, `Build from recorded audio: ${a.name}`)) },
          `${a.name} · ${a.secs.toFixed(1)}${tr(" 秒", " s")}`,
          thumbCanvas({ kind: "audio", id: a.id, range: 1 }, true),
        ),
      ),
    );
    const audioPanel = panel(
      showAudio && !WT.showShelf,
      el("div", { class: "hint" }, tr("曲の中の音声のクリップから、1 周期ずつ切り出して順に並べます(高さは自動で測る)。", "Cuts one cycle at a time from an audio clip in the song and lines them up (pitch is detected automatically).")),
      clips,
      d.src.kind === "audio"
        ? knob("使う範囲", {
            min: 0.05,
            max: 1,
            step: 0.01,
            value: d.src.range ?? 1,
            fmt: (v) => `${Math.round(v * 100)}%`,
            title: tr("素材の頭から、どこまでを使うか", "How much of the source to use, from its start"),
            onInput: (v) => {
              d.src.range = v;
            },
            onCommit: (v) => {
              d.src.range = v;
              pushHist(tr(`使う範囲を ${Math.round(v * 100)}% に`, `Range → ${Math.round(v * 100)}%`));
              rerender();
            },
          })
        : null,
    );
    const shelf = el("div", { class: "shelf" });
    if (shelfNames == null) {
      shelf.append(el("div", { class: "hint" }, tr("読み込み中…", "Loading…")));
      api
        .wavetableLibrary()
        .then((l) => {
          shelfNames = l;
          if (WT.showShelf) rerender();
        })
        .catch(() => (shelfNames = []));
    } else if (!shelfNames.length) shelf.append(el("div", { class: "hint" }, tr("まだありません", "Nothing yet")));
    (shelfNames ?? []).forEach((s) =>
      shelf.append(
        el(
          "div",
          { class: "shelfitem", title: tr("読み込む(1 回の取り消しで戻る)", "Load (one undo brings it back)"), onclick: () => setSrc({ kind: "library", name: s.name }, tr(`保存した波形「${s.name}」を読み込む`, `Load saved table "${s.name}"`)) },
          s.name,
          WT.showShelf ? thumbCanvas({ kind: "library", name: s.name }) : null,
        ),
      ),
    );
    const shelfPanel = panel(
      WT.showShelf,
      el("div", { class: "hint" }, tr("右下の「保存する」で残したウェーブテーブル。全部の曲で使えます。配布のウェーブテーブル(Serum などの WAV)を置いても並びます。", "Wavetables saved with “Save” at the bottom right; usable in every song. Distributed wavetables (Serum WAVs etc.) placed there show up too.")),
      shelf,
    );
    srcBox.append(shapePanel, harmPanel, audioPanel, shelfPanel);
    const frames = el("select", {
      class: "trackpick",
      onchange: (e: Event) => {
        if (!canEdit()) return;
        d.frames = +(e.target as HTMLSelectElement).value;
        pushHist(tr(`波形の数を ${d.frames} に`, `Frame count → ${d.frames}`));
        rerender();
      },
    });
    [16, 32, 64, 128, 256].forEach((n) => frames.append(el("option", { value: n, selected: n === d.frames }, `${n}`)));
    srcBox.append(el("div", { class: "row small" }, el("span", { class: "dim", title: tr("位置 0〜1 の間に並べる波形の数。多いほど変化がなめらか", "Number of frames between positions 0–1. More = smoother changes") }, tr("波形の数", "Frames")), frames));
    left.append(srcBox);

    // ---- 真ん中: 見る・描く(画面の高さいっぱい) ----
    const center = el("div", { class: "col center wtcol" });
    const grid = el("div", { class: "wtgrid" });
    // 1. 波形の移り変わり(位置のつまみのすぐ下に、編集した波形の印を並べる)
    const stackC = el("canvas", { id: "wtStack", class: "fill", title: tr("0%(手前)〜 100%(奥)の波形の並び。クリックで調整する波形を選ぶ。白い線が下で調整している波形", "Frames from 0% (front) to 100% (back). Click to pick the frame to edit. The white line is the one edited below") });
    stackC.addEventListener("pointerdown", (e) => {
      if (!canEdit()) return;
      const r = stackC.getBoundingClientRect();
      d.pos = clamp(1 - (e.clientY - r.top - 32) / (r.height - 44), 0, 1);
      pushHist(tr(`調整する波形を ${Math.round(d.pos * 100)}% に`, `Edited frame → ${Math.round(d.pos * 100)}%`));
      rerender();
    });
    const pos = el("input", { type: "range", min: 0, max: 1, step: 0.001, value: d.pos, id: "wtPos" });
    pos.addEventListener("input", () => {
      d.pos = +pos.value;
      drawAll();
      WT.updateEditHead();
    });
    pos.addEventListener("change", () => pushHist(tr(`位置を ${Math.round(d.pos * 100)}% に`, `Position → ${Math.round(d.pos * 100)}%`)));
    const blendIn = el("input", { type: "range", min: 0, max: 0.5, step: 0.01, value: d.blend ?? 0.15, title: tr("編集した波形の前後のどこまでを、編集した形へ少しずつ寄せるか。0 にすると、その 1 枚だけが変わる", "How far around an edited frame to gradually blend toward it. At 0 only that one frame changes") });
    blendIn.addEventListener("input", () => {
      d.blend = +blendIn.value;
      const o = byId("wtBlendOut");
      if (o) o.textContent = `${Math.round(d.blend * 100)}%`;
      drawAll();
      renderKeys();
    });
    blendIn.addEventListener("change", () => pushHist(tr(`なじませる幅を ${Math.round(d.blend * 100)}% に`, `Blend width → ${Math.round(d.blend * 100)}%`)));
    const keysBar = el("div", { class: "keys", id: "wtKeys" });
    const harmSrc = d.src.kind === "harmonics";
    grid.append(
      el(
        "div",
        { class: "box", style: "grid-column: span 4" },
        el("h3", {}, tr("波形の移り変わり", "Frames"), el("span", { class: "spacer" }), el("span", { class: "hint", title: tr("鳴っている間は LFO で動く(LFO の深さはインスペクターで)", "While sounding it moves with the LFO (LFO depth is in the inspector)") }, tr("手前が 0%、奥が 100%。白い線 = 下で調整している波形", "Front = 0%, back = 100%. White line = the frame edited below"))),
        stackC,
        el("div", { class: "posrow" }, el("span", { class: "dim" }, tr("調整する波形を選ぶ", "Frame to edit")), pos, el("output", { id: "wtPosOut" })),
        el("div", { class: "posrow" }, el("span", { class: "dim" }, tr("編集したところ", "Edited frames")), keysBar, el("span", {})),
        el(
          "div",
          { class: "posrow" },
          el("span", {}),
          el(
            "div",
            { class: "legendrow" },
            el("span", {}, el("span", { class: "sw ed" }), tr("編集した", "Edited")),
            el(
              "span",
              { title: harmSrc ? tr("前後の編集した波形を、位置に合わせて少しずつ混ぜて作った形", "Made by gradually mixing the edited frames before and after") : tr("作り方の波形を、近くの編集した波形へ少しずつ寄せた形", "The source frame gradually pulled toward a nearby edited frame") },
              el("span", { class: "sw auto" }),
              harmSrc ? tr("自動(前後を混ぜた形)", "Auto (mixed)") : tr("自動(編集になじませた形)", "Auto (blended)"),
            ),
            harmSrc ? null : el("span", {}, el("span", { class: "sw src" }), tr("作り方のまま", "As made")),
            el("span", {}, el("span", { class: "sw now" }), tr("調整している波形", "Frame being edited")),
            harmSrc ? null : el("label", { style: "margin-left:auto" }, tr("なじませる幅", "Blend width"), blendIn, el("output", { id: "wtBlendOut" }, `${Math.round((d.blend ?? 0.15) * 100)}%`)),
          ),
        ),
      ),
    );
    // 2. 鳴っている音
    grid.append(specBox("grid-column: span 2"));
    // 3. 調整: 今の位置の波形と倍音を 1 つの枠に。道具はそれぞれの欄の中
    // 音域(凡例の音名も変わるので描き直す)
    const prev = pitchPick(d, () => rerender());
    const waveTool = el("div", { class: "seg" });
    (
      [
        ["view", tr("見るだけ", "View"), tr("見るだけ(調整しない)", "View only (no editing)")],
        ["points", tr("点をつまむ", "Drag points"), tr("波形の上の点をつまんで上下に動かす", "Drag the points on the waveform up and down")],
        ["pencil", tr("線で描く", "Draw"), tr("マウスで線を引いて波形を描く", "Draw the waveform with the mouse")],
      ] as [string, string, string][]
    ).forEach(([k, n, tip]) =>
      waveTool.append(
        el(
          "button",
          {
            class: d.tool === k || (k === "view" && d.tool === "harm") ? "on" : "",
            title: tip,
            onclick: () => {
              d.tool = k;
              rerender();
            },
          },
          ico(k),
          n,
        ),
      ),
    );
    const harmTool = el("div", { class: "seg" });
    (
      [
        ["view", tr("見るだけ", "View"), tr("見るだけ(調整しない)", "View only (no editing)")],
        ["harm", tr("倍音を描く", "Draw harmonics"), tr("棒を上下にドラッグして、倍音の強さを描く", "Drag bars up and down to draw harmonic levels")],
      ] as [string, string, string][]
    ).forEach(([k, n, tip]) =>
      harmTool.append(
        el(
          "button",
          {
            class: (k === "harm" ? d.tool === "harm" : d.tool !== "harm") ? "on" : "",
            title: tip,
            onclick: () => {
              d.tool = k;
              rerender();
            },
          },
          ico(k),
          n,
        ),
      ),
    );
    const waveC = el("canvas", { id: "wtWave", class: "fill", title: tr("調整している位置の波形", "The frame being edited") });
    const harmC = el("canvas", { id: "wtHarm", class: "fill", title: tr("調整している位置の倍音の強さ", "Harmonic levels of the frame being edited") });
    const R = rangeOf(d);
    grid.append(
      el(
        "div",
        { class: "box editgroup", style: "grid-column: span 6" },
        el(
          "div",
          { class: "edithead" },
          el("span", { class: "nowmark", title: tr("上の「波形の移り変わり」の白い線の波形", "The white-line frame in “Frames” above") }),
          el("b", { id: "wtEditTitle" }, tr("白い線の波形を調整", "Edit the white-line frame")),
          el(
            "button",
            {
              class: "btn quiet",
              id: "wtDelKey",
              title: tr("この波形の編集を消す(作り方の形・自動の形に戻る)", "Remove this frame's edit (back to the made / auto shape)"),
              onclick: () => {
                if (!canEdit()) return;
                const i = d.keys.indexOf(wtKeyAt(d, d.pos));
                if (i < 0) return;
                d.keys.splice(i, 1);
                pushHist(tr(`${Math.round(d.pos * 100)}% の編集した波形を消す`, `Remove the edited frame at ${Math.round(d.pos * 100)}%`));
                rerender();
              },
            },
            tr("この編集を消す", "Remove this edit"),
          ),
          el("span", { class: "spacer" }),
          prev,
        ),
        el(
          "div",
          { class: "editpanes" },
          el(
            "div",
            { class: "pane" },
            el("div", { class: "panehead" }, el("span", {}, tr("波形", "Waveform"))),
            el("div", { class: "toolseg" }, waveTool),
            waveC,
            // 凡例: 音域の中の段ごとの波形(エンジンは倍音の数を段で切り替える)と、描いた形
            el(
              "div",
              { class: "legendrow", style: "min-height: 1lh" },
              ...wtSteps(R.lo, R.hi).map((st, i) =>
                el(
                  "span",
                  {
                    style: "cursor: default",
                    title: tr(`${noteSpan(st.from, st.to)} の鍵盤で鳴る波形(倍音 ${st.k} 本)。マウスを置くと、この段だけ太く`, `The waveform played on ${noteSpan(st.from, st.to)} (${st.k} harmonics). Hover to emphasize this step`),
                    onmouseenter: () => {
                      WT.hiStep = i;
                      WT.drawWave();
                      WT.drawHarm();
                    },
                    onmouseleave: () => {
                      WT.hiStep = null;
                      WT.drawWave();
                      WT.drawHarm();
                    },
                  },
                  el("i", { class: "legend-line", style: `border-top: 2.5px solid ${wtStepColor(i)}` }),
                  `${noteSpan(st.from, st.to)}`,
                ),
              ),
              el("span", {}, el("i", { class: "legend-line", style: "border-top: 1px dashed var(--human)" }), tr("描いた形", "Drawn shape")),
            ),
          ),
          el(
            "div",
            { class: "pane" },
            el("div", { class: "panehead" }, el("span", {}, tr("倍音", "Harmonics"))),
            el("div", { class: "toolseg" }, harmTool),
            harmC,
            el("span", { class: "hint", id: "wtHarmInfo", style: "min-height: 1lh" }, ""),
          ),
        ),
        el("div", { class: "hint", id: "wtToolHint" }, WT.toolHint(d)),
      ),
    );
    center.append(grid);

    // ---- 描く操作 ----
    let drawing: string | null = null,
      lastX: number | null = null,
      lastY = 0,
      drawn: Float32Array | null = null,
      ptIdx: number | null = null,
      pts: number[] | null = null;
    const moveWave = (e: PointerEvent) => {
      const r = waveC.getBoundingClientRect();
      const fx = clamp((e.clientX - r.left) / r.width, 0, 0.9999),
        fy = (e.clientY - r.top) / r.height;
      if (!drawing) {
        // 点の上ではつかめることを見せる
        if (d.tool === "points") {
          const hp = WT.handles();
          const near = hp ? hp.findIndex((y: number, i: number) => Math.hypot((i / PTS - fx) * r.width, (0.5 - y * 0.45 - fy) * r.height) < 22) : -1;
          waveC.style.cursor = near >= 0 ? "grab" : "default";
          WT.hoverPt = near;
          WT.drawWave();
        }
        return;
      }
      if (drawing === "points" && pts && ptIdx != null) {
        waveC.style.cursor = "grabbing";
        pts[ptIdx] = clamp((0.5 - fy) / 0.45, -1.1, 1.1);
        drawn = splineWave(pts);
        WT.liveKey(drawn, pts);
      } else if (drawing === "pencil" && drawn) {
        const n = Math.floor(fx * N),
          v = clamp((0.5 - fy) / 0.45, -1.1, 1.1);
        if (lastX == null) {
          lastX = n;
          lastY = v;
        }
        const a = Math.min(lastX, n),
          b = Math.max(lastX, n);
        for (let i = a; i <= b; i++) drawn[i] = b === a ? v : lastY + ((v - lastY) * (i - lastX)) / (n - lastX || 1);
        lastX = n;
        lastY = v;
        WT.liveKey(drawn, null);
      }
    };
    waveC.addEventListener("pointerdown", (e) => {
      if (d.tool !== "points" && d.tool !== "pencil") return;
      if (!canEdit()) return;
      const pre = WT.pre(d);
      if (!pre) return;
      const r = waveC.getBoundingClientRect();
      const fx = (e.clientX - r.left) / r.width,
        fy = (e.clientY - r.top) / r.height;
      if (d.tool === "points") {
        pts = WT.handles();
        if (!pts) return;
        const best = pts.map((y, i) => [i, Math.hypot((i / PTS - fx) * r.width, (0.5 - y * 0.45 - fy) * r.height)] as [number, number]).sort((a, b) => a[1] - b[1])[0];
        if (best[1] > 22) {
          pts = null;
          return;
        }
        ptIdx = best[0];
        drawing = "points";
      } else {
        drawing = "pencil";
        // 描くのは加工の前の波形
        drawn = Float32Array.from(pre.x);
        lastX = null;
      }
      waveC.setPointerCapture(e.pointerId);
      moveWave(e);
    });
    waveC.addEventListener("pointermove", moveWave);
    waveC.addEventListener("pointerup", () => {
      if (!drawing) return;
      pushHist(drawing === "points" ? tr(`${Math.round(d.pos * 100)}% の波形の点を動かす`, `Move points of the frame at ${Math.round(d.pos * 100)}%`) : tr(`${Math.round(d.pos * 100)}% の波形を線で描く`, `Draw the frame at ${Math.round(d.pos * 100)}%`));
      drawing = null;
      drawn = null;
      pts = null;
      ptIdx = null;
      WT.drawnLive = null;
      rerender();
    });
    waveC.addEventListener("pointerleave", () => {
      WT.hoverPt = null;
      if (!drawing) WT.drawWave();
    });
    // 倍音を描く
    let harmDraw = false;
    const moveHarm = (e: PointerEvent) => {
      const r = harmC.getBoundingClientRect();
      const L = WT.harmLayout(r.width, r.height);
      const fx = e.clientX - r.left,
        fy = e.clientY - r.top;
      const k = clamp(Math.floor((fx - L.x0) / L.bw), 0, HB - 1);
      WT.hoverK = fx >= L.x0 ? k : null;
      if (harmDraw) {
        const key = WT.ensureKeyHere();
        if (key) {
          const m = Math.max(...key.amps.slice(0, HB)) || 1;
          key.amps[k] = clamp((L.bot - fy) / (L.bot - L.top), 0, 1) * m;
          key.ph = null;
          drawAll();
          return;
        }
      }
      WT.drawHarm();
    };
    harmC.addEventListener("pointerdown", (e) => {
      if (d.tool !== "harm" || !canEdit() || !WT.pre(d)) return;
      harmDraw = true;
      harmC.setPointerCapture(e.pointerId);
      moveHarm(e);
    });
    harmC.addEventListener("pointermove", moveHarm);
    harmC.addEventListener("pointerup", () => {
      if (harmDraw) {
        pushHist(tr(`${Math.round(d.pos * 100)}% の ${(WT.hoverK ?? 0) + 1} 番目の倍音を描く`, `Draw harmonic ${(WT.hoverK ?? 0) + 1} of the frame at ${Math.round(d.pos * 100)}%`));
        harmDraw = false;
        rerender();
      }
    });
    harmC.addEventListener("pointerleave", () => {
      WT.hoverK = null;
      WT.drawHarm();
    });

    // ---- 右: 加工の手順・保存 ----
    const right = el("div", { class: "col right" });
    const ops = el("div", { class: "ops", id: "wtOps" });
    d.ops.forEach((op: Data, i: number) => ops.append(opCard(op, i)));
    const menu = el("div", { class: "menu" });
    Object.entries(OPS()).forEach(([k, o]) =>
      menu.append(
        el(
          "button",
          {
            class: "opitem",
            onclick: () => {
              menu.classList.remove("open");
              if (!canEdit()) return;
              const p: Data = {};
              o.params.forEach(([n, , , , , def]) => (p[n] = def));
              const id = Math.max(0, ...d.ops.map((x: Data) => Number(x.id) || 0)) + 1;
              d.ops.push({ id, type: k, on: true, who: "you", p });
              pushHist(tr(`加工「${o.name}」を足す`, `Add "${o.name}"`));
              rerender();
            },
          },
          ico(opIcon(k)),
          el("span", {}, o.name, el("small", {}, o.desc)),
        ),
      ),
    );
    right.append(
      el(
        "div",
        { class: "box" },
        el("h3", {}, tr("加工の手順", "Processing"), el("span", { class: "spacer" }), el("span", { class: "hint" }, tr("上から順に掛かる", "Applied top to bottom"))),
        el(
          "div",
          { class: "hint" },
          tr(
            "作り方で作った波形に、上から順に加工を掛けます。AI が足した加工(紫)も同じ所に並び、続きを人が調整できます。⠿ をつかんで並べ替え、チェックで入り切り。",
            "Processing is applied top to bottom to the made frames. Steps the AI added (purple) appear here too, and you can keep tweaking them. Drag ⠿ to reorder; the checkbox turns a step on or off.",
          ),
        ),
        ops,
        el(
          "div",
          { class: "addop" },
          el(
            "button",
            {
              class: "btn",
              style: "width:100%",
              onclick: (e: Event) => {
                e.stopPropagation();
                menu.classList.toggle("open");
              },
            },
            tr("+ 加工を足す", "+ Add processing"),
          ),
          menu,
        ),
      ),
    );
    document.addEventListener("click", () => menu.classList.remove("open"), { once: true });
    const saveName = el("input", { type: "text", placeholder: tr("名前", "Name"), value: state.trackName });
    right.append(
      el(
        "div",
        { class: "box" },
        el("h3", {}, tr("保存(全部の曲で使える)", "Save (for every song)")),
        el(
          "div",
          { class: "hint" },
          tr(
            "このウェーブテーブルを、ほかの曲でも使えるように残します(作り方と加工の手順も一緒に。読み込めば続きを調整できる)。読み込みは左の「保存した波形」から。",
            "Keeps this wavetable for other songs (with its recipe and processing; load it to keep editing). Load it from “Saved tables” on the left.",
          ),
        ),
        el(
          "div",
          { class: "row" },
          saveName,
          el(
            "button",
            {
              class: "btn",
              onclick: async () => {
                const n = saveName.value.trim();
                if (!n) return;
                try {
                  await api.wavetableLibrarySave(state.track, n, recipeOf(cur()));
                  shelfNames = null;
                  toast(tr(`「${n}」として保存しました(全部の曲で使えます)`, `Saved as "${n}" (usable in every song)`));
                } catch (e) {
                  showToast("error", tr(`保存できませんでした: ${e}`, `Couldn't save: ${e}`));
                }
              },
            },
            tr("保存する", "Save"),
          ),
        ),
        el(
          "div",
          { class: "row" },
          el(
            "button",
            {
              class: "btn",
              onclick: async () => {
                const path = await pickSave({ title: tr("ウェーブテーブルを WAV に書き出す", "Export the wavetable as WAV"), defaultPath: `${saveName.value.trim() || "wavetable"}.wav`, filters: [{ name: "WAV", extensions: ["wav"] }] });
                if (typeof path !== "string") return;
                try {
                  await api.wavetableExport(state.track, recipeOf(cur()), path);
                  toast(tr("WAV に書き出しました(1 周期 2048 点・Serum などで読める)", "Exported as WAV (2048 points per cycle; readable by Serum etc.)"));
                } catch (e) {
                  showToast("error", tr(`書き出せませんでした: ${e}`, `Couldn't export: ${e}`));
                }
              },
            },
            tr("WAV に書き出す", "Export WAV"),
          ),
          el("span", { class: "hint" }, tr("ほかのシンセで使うとき", "For use in other synths")),
        ),
      ),
    );
    body.append(left, center, right);
    requestAnimationFrame(() => renderKeys());

    function renderKeys() {
      // 位置 0〜1 の帯を、波形 1 枚ずつ塗り分ける: 編集した = 塗りつぶし、自動 = 斜線、作り方のまま = 塗らない。
      // 見るためのもの(クリックでその位置へ移るだけ。波形は変わらない)
      const bar = byId("wtKeys");
      if (!bar) return;
      bar.innerHTML = "";
      const F = d.frames;
      if (!d.keys.length) bar.append(el("span", { class: "lbl", title: tr("下の枠で波形を調整すると、ここが塗られます", "Editing a frame below fills this in") }, tr("まだ無し(調整すると塗られる)", "None yet (fills in when you edit)")));
      // 自動の所: 続いている所をまとめて 1 本の斜線に
      let runStart: number | null = null;
      for (let i = 0; i <= F; i++) {
        const isAuto = i < F && frameKind(d, wtSrcPos(d, F > 1 ? i / (F - 1) : 0), F) !== "src";
        if (isAuto && runStart == null) runStart = i;
        if (!isAuto && runStart != null) {
          const a = Math.max(0, (runStart - 0.5) / (F - 1)),
            b = Math.min(1, (i - 0.5) / (F - 1));
          bar.append(el("div", { class: "auto", style: `left:${keyLeft(a)};width:calc(${b - a} * (100% - 20px))` }));
          runStart = null;
        }
      }
      d.keys.forEach((k: Data) => {
        // 切り出しの外の編集は表に出ない
        const o = wtOutPos(d, k.pos);
        if (o == null) return;
        const m = el("div", {
          class: "ed" + (k.who === "ai" ? " ai" : ""),
          style: `left:${keyLeft(o)};width:calc(${1 / (F - 1)} * (100% - 20px))`,
          title: tr(`${Math.round(o * 100)}%: ${k.who === "ai" ? "AI が" : ""}編集した波形。クリックでこの波形へ`, `${Math.round(o * 100)}%: frame edited by ${k.who === "ai" ? "the AI" : "you"}. Click to go to it`),
        });
        m.addEventListener("click", (e) => {
          e.stopPropagation();
          d.pos = o;
          pushHist(tr(`調整する波形を ${Math.round(o * 100)}% に`, `Edited frame → ${Math.round(o * 100)}%`));
          setSel(tr(`編集した波形 ${Math.round(k.pos * 100)}%`, `Edited frame ${Math.round(k.pos * 100)}%`));
          rerender();
        });
        bar.append(m);
      });
      bar.append(el("div", { class: "now", style: `left:${keyLeft(d.pos)}` }));
      bar.onclick = (e) => {
        const r = bar.getBoundingClientRect();
        d.pos = clamp((e.clientX - r.left - 10) / (r.width - 20), 0, 1);
        pushHist(tr(`調整する波形を ${Math.round(d.pos * 100)}% に`, `Edited frame → ${Math.round(d.pos * 100)}%`));
        rerender();
      };
    }
    function opCard(op: Data, i: number) {
      const o = OPS()[op.type] ?? { name: String(op.raw?.op ?? "?"), desc: "", params: [] };
      const card = el("div", {
        class: "op " + op.who + (op.on ? "" : " off"),
        onclick: () => {
          d.selOp = op.id;
          setSel(tr(`加工「${o.name}」`, `Processing "${o.name}"`));
        },
      });
      const grip = el("span", { class: "grip", title: tr("つかんで並べ替え", "Drag to reorder") }, "⠿");
      const cb = el("input", {
        type: "checkbox",
        checked: op.on,
        title: tr("入り切り(切ると、この加工を飛ばして聴ける)", "On / off (turn off to hear without this step)"),
        onchange: (e: Event) => {
          if (!canEdit()) return;
          op.on = (e.target as HTMLInputElement).checked;
          pushHist(tr(`加工「${o.name}」を${op.on ? "入れる" : "切る"}`, `${op.on ? "Turn on" : "Turn off"} "${o.name}"`));
          rerender();
        },
      });
      card.append(
        el(
          "div",
          { class: "oh" },
          grip,
          cb,
          ico(opIcon(op.type)),
          el("span", { class: "nm" }, `${i + 1}. ${o.name}`),
          el("span", { class: "pill " + op.who }, op.who === "ai" ? "AI" : tr("人", "You")),
          el(
            "button",
            {
              class: "x",
              title: tr("消す", "Remove"),
              onclick: (e: Event) => {
                e.stopPropagation();
                if (!canEdit()) return;
                d.ops.splice(i, 1);
                pushHist(tr(`加工「${o.name}」を消す`, `Remove "${o.name}"`));
                rerender();
              },
            },
            "×",
          ),
        ),
      );
      // 位置ごとの効き方(AI が fade で付けたもの)
      if (Array.isArray(op.fade))
        card.append(
          el(
            "div",
            { class: "hint", title: tr("テーブルの位置によって効き方が変わる(位置 0 → 位置 1)", "The effect strength changes along the table position (position 0 → 1)") },
            tr(`位置で効き方が変わる: ${Math.round(op.fade[0] * 100)}% → ${Math.round(op.fade[1] * 100)}%`, `Strength by position: ${Math.round(op.fade[0] * 100)}% → ${Math.round(op.fade[1] * 100)}%`),
          ),
        );
      o.params.forEach(([n, label, min, max, step]) =>
        card.append(
          knob(label, {
            min,
            max,
            step,
            value: op.p[n],
            fmt: n === "mode" ? (v: number) => [tr("そろえる", "zero"), tr("波形どうし", "align"), tr("ばらす", "random")][clamp(Math.round(v), 0, 2)] : undefined,
            onInput: (v) => {
              op.p[n] = v;
              drawAll();
            },
            onCommit: (v) => pushHist(tr(`加工「${o.name}」の${label}を ${v} に`, `"${o.name}" ${label} → ${v}`)),
          }),
        ),
      );
      grip.addEventListener("pointerdown", (e) => {
        e.preventDefault();
        if (!canEdit()) return;
        grip.setPointerCapture(e.pointerId);
        card.classList.add("drag");
        const cards = [...(byId("wtOps")?.children ?? [])];
        const mids = cards.map((c) => {
          const r = c.getBoundingClientRect();
          return r.top + r.height / 2;
        });
        let to = i;
        const mv = (ev: PointerEvent) => {
          to = mids.filter((m, j) => j !== i && m < ev.clientY).length;
        };
        const up = () => {
          grip.removeEventListener("pointermove", mv);
          grip.removeEventListener("pointerup", up);
          card.classList.remove("drag");
          if (to !== i) {
            const [x] = d.ops.splice(i, 1);
            d.ops.splice(to, 0, x);
            pushHist(tr(`加工「${o.name}」を ${to + 1} 番目へ`, `Move "${o.name}" to #${to + 1}`));
            rerender();
          }
        };
        grip.addEventListener("pointermove", mv);
        grip.addEventListener("pointerup", up);
      });
      return card;
    }
  },
  /** その位置に編集した波形が無ければ、加工の前の波形から作る(加工の後から作ると、加工が二重に掛かる) */
  ensureKeyHere() {
    const d = cur();
    let k = wtKeyAt(d, d.pos);
    if (!k) {
      const pre = WT.pre(d);
      if (!pre) return null;
      const sp = wtSrcPos(d, d.pos);
      k = { pos: wtIdx(d, sp) / Math.max(1, d.frames - 1), amps: pre.amps.slice(0, HK), ph: pre.ph.slice(0, HK), pts: null, who: "you" };
      d.keys.push(k);
    }
    d.selKey = d.keys.indexOf(k);
    setSel(tr(`編集した波形 ${Math.round(k.pos * 100)}%`, `Edited frame ${Math.round(k.pos * 100)}%`));
    return k;
  },
  toolHint(d: Data) {
    return (
      {
        view: tr(
          "上の白い線の波形(「調整する波形を選ぶ」か、絵・帯のクリックで選ぶ)を、波形の欄の「点をつまむ」「線で描く」か、倍音の欄の「倍音を描く」で調整できます。調整した波形は、上の「編集したところ」の帯が塗られます。",
          "Edit the white-line frame above (pick it with “Frame to edit” or by clicking the picture or the strip) using “Drag points” / “Draw” in the waveform pane or “Draw harmonics” in the harmonics pane. Edited frames fill in the “Edited frames” strip above.",
        ),
        points: tr("波形の上の点(○)をつまんで上下に動かします。太い線が実際に鳴る形です(右上の「音域」で出せる倍音が変わり、高い音ほど丸くなります)。", "Drag the points (○) up and down. The thick line is the shape actually played (the range at the top right changes how many harmonics fit; higher notes get rounder)."),
        pencil: tr("マウスで線を引いて、波形を自由に描きます。点線が描いた形、太い線が実際に鳴る形。角や細かいギザギザは、鳴らせる倍音の範囲で丸くなります。", "Draw the waveform freely with the mouse. Dotted = what you drew, thick = what is played. Corners and fine jaggies get rounded to the harmonics that can be played."),
        harm: tr(
          "棒(1 本 = 1 つの倍音)を上下にドラッグ。描いている間の棒は加工を掛ける前の倍音です(波形の欄の太い線は、加工を掛けた後の鳴る形)。マウスの下の倍音の番号・周波数・強さは絵の下に出ます。灰色の棒は、この音域では鳴らない倍音。",
          "Drag bars (one bar = one harmonic) up and down. While drawing, the bars show harmonics before processing (the thick line in the waveform pane is the processed, played shape). The harmonic under the mouse, its Hz and level appear below. Gray bars aren't played in this range.",
        ),
      } as Record<string, string>
    )[d.tool];
  },
  /** 調整の枠の見出し: その位置に編集した波形があるときだけ「この編集を消す」を出す */
  updateEditHead() {
    const d = cur();
    const k = wtKeyAt(d, d.pos);
    const del = byId("wtDelKey");
    if (del) del.hidden = !k;
  },
  /** 今の位置の波形の 16 点(つまむ点。加工の前の波形の上) */
  handles(): number[] | null {
    const d = cur();
    const k = wtKeyAt(d, d.pos);
    if (k && k.pts) return [...k.pts];
    const pre = WT.pre(d);
    if (!pre) return null;
    return Array.from({ length: PTS }, (_, i) => pre.x[Math.round((i / PTS) * N) % N]);
  },
  /** 描いている途中: 描いた形をその場で今の位置の波形にする */
  liveKey(drawn: Float32Array, pts: number[] | null) {
    const key = WT.ensureKeyHere();
    if (!key) return;
    const { amps, ph } = dft(drawn);
    key.amps = amps;
    key.ph = ph;
    key.pts = pts ? [...pts] : null;
    WT.drawnLive = drawn;
    drawAll();
  },
  livePos() {
    const d = shown();
    if (!WT.held || !listen.moveMod) return d.pos;
    return clamp(d.pos + d.lfo * 0.5 * Math.sin(WT.lfoT), 0, 1);
  },
  /** その鍵盤で出せる倍音だけにした波形(実際に鳴る形) */
  played(f: api.WtFrame, note: number) {
    return idft(f.amps, f.phases, audibleHarmonics(note));
  },
  draw() {
    const d = shown();
    if (!d) return;
    const c = byId<HTMLCanvasElement>("wtStack");
    if (!c) return;
    const v = WT.view(d);
    const { g, w, h } = ctx2d(c);
    if (!v) {
      g.fillStyle = css("--faint");
      g.font = "11px sans-serif";
      g.fillText(tr("テーブルを作っています…", "Building the table…"), 10, h / 2);
      WT.drawWave();
      WT.drawHarm();
      return;
    }
    const t = v.stack,
      F = v.frames,
      show = t.length;
    const live = WT.livePos();
    const top = 14,
      depth = h - 44,
      dx = Math.min(120, w * 0.15);
    for (let j = show - 1; j >= 0; j--) {
      const f = show > 1 ? j / (show - 1) : 0;
      const y0 = top + depth * (1 - f) + 18,
        x0 = 10 + dx * f,
        ww = w - dx - 30;
      const tt = f;
      const near = Math.abs(tt - d.pos) < 0.5 / show;
      const playing = live !== d.pos && Math.abs(tt - live) < 0.5 / show;
      const kind = frameKind(d, wtSrcPos(d, tt), d.frames);
      const edited =
        kind === "ed" ||
        d.keys.some((k: Data) => {
          const o = wtOutPos(d, k.pos);
          return o != null && Math.abs(o - tt) < 0.5 / show;
        });
      g.strokeStyle = near ? "#ffffff" : playing ? "#e8c46a" : edited ? css("--accent") : kind === "auto" ? `rgba(37,189,177,${0.18 + 0.3 * (1 - f)})` : `rgba(200,200,200,${0.15 + 0.5 * (1 - f)})`;
      g.lineWidth = near || edited || playing ? 2 : 1;
      g.beginPath();
      const x = t[j];
      for (let n = 0; n < x.length; n++) {
        const xx = x0 + (n / x.length) * ww,
          y = y0 - x[n] * Math.min(26, depth / 6);
        if (n) g.lineTo(xx, y);
        else g.moveTo(xx, y);
      }
      g.stroke();
    }
    void F;
    // 調整している位置の線に名札(下の枠とつなぐ)
    if (live !== d.pos) {
      const f = clamp(live, 0, 1),
        y0 = top + depth * (1 - f) + 18,
        x0 = 10 + dx * f;
      const tw0 = tr("鳴っている所(LFO で動く)", "Now playing (moves with the LFO)");
      g.font = "11px sans-serif";
      g.fillStyle = "rgba(0,0,0,0.78)";
      const lw0 = g.measureText(tw0).width;
      g.fillRect(x0 - 7, y0 - 21, lw0 + 6, 15);
      g.fillStyle = "#e8c46a";
      g.fillText(tw0, x0 - 4, y0 - 10);
    }
    {
      const f = clamp(d.pos, 0, 1),
        y0 = top + depth * (1 - f) + 18,
        x0 = 10 + dx * f + (w - dx - 30);
      const label = tr(`← 下で調整している波形(${Math.round(d.pos * 100)}%)`, `← frame edited below (${Math.round(d.pos * 100)}%)`);
      g.font = "11px sans-serif";
      const lw = g.measureText(label).width,
        lx = Math.min(x0 + 4, w - lw - 6);
      g.fillStyle = "rgba(0,0,0,0.75)";
      g.fillRect(lx - 3, y0 - 8, lw + 6, 16);
      g.fillStyle = "#ffffff";
      g.fillText(label, lx, y0 + 4);
    }
    g.fillStyle = css("--dim");
    g.font = "11px sans-serif";
    g.fillText(tr("0%(手前)", "0% (front)"), 10, h - 4);
    g.fillText(tr("100%(奥)", "100% (back)"), 10 + dx, top);
    const po = byId("wtPosOut");
    if (po) po.textContent = Math.round(d.pos * 100) + "%";
    WT.updateEditHead();
    const pin = byId<HTMLInputElement>("wtPos");
    if (pin) pin.value = String(d.pos);
    WT.drawWave();
    WT.drawHarm();
    const nowBar = byId("wtKeys")?.querySelector<HTMLElement>(".now");
    if (nowBar) nowBar.style.left = keyLeft(d.pos);
    ui.panes.forEach((p) => p.draw());
  },
  drawWave() {
    const c = byId<HTMLCanvasElement>("wtWave");
    if (!c) return;
    const d = shown();
    const { g, w, h } = ctx2d(c);
    const v = WT.view(d);
    const yOf = (val: number) => h / 2 - val * h * 0.45;
    g.strokeStyle = "#2a2a2a";
    g.lineWidth = 1;
    [-1, -0.5, 0, 0.5, 1].forEach((val) => {
      g.beginPath();
      g.moveTo(0, yOf(val));
      g.lineTo(w, yOf(val));
      g.stroke();
    });
    // 描いた形(細い線)
    const hp = d.tool === "points" ? WT.handles() : null;
    const drawnShape = WT.drawnLive ?? (hp ? splineWave(hp) : null);
    if (drawnShape && d.tool !== "view" && d.tool !== "harm") {
      g.strokeStyle = css("--human");
      g.lineWidth = 1;
      g.setLineDash([3, 3]);
      g.beginPath();
      for (let n = 0; n < N; n++) {
        const x = (n / N) * w,
          y = yOf(drawnShape[n]);
        if (n) g.lineTo(x, y);
        else g.moveTo(x, y);
      }
      g.stroke();
      g.setLineDash([]);
    }
    // 波形: 音域の中の段ごと(エンジンは倍音の数を段で切り替える)。強調した段はいちばん上に、いちばん低い段は太く
    if (v) {
      const R = rangeOf(d),
        steps = wtSteps(R.lo, R.hi);
      const hi = WT.hiStep != null && WT.hiStep < steps.length ? WT.hiStep : null;
      const order = steps.map((_, i) => i).reverse();
      if (hi != null) {
        order.splice(order.indexOf(hi), 1);
        order.push(hi);
      }
      order.forEach((i) => {
        const xh = WT.played(v.post, steps[i].from);
        g.globalAlpha = hi == null || hi === i ? 1 : 0.22;
        g.strokeStyle = wtStepColor(i);
        g.lineWidth = hi === i ? 3 : i === 0 ? 2.5 : 1.6;
        g.beginPath();
        for (let n = 0; n < N; n++) {
          const xx = (n / N) * w,
            y = yOf(xh[n]);
          if (n) g.lineTo(xx, y);
          else g.moveTo(xx, y);
        }
        g.stroke();
        g.globalAlpha = 1;
      });
    }
    // つまむ点
    if (d.tool === "points" && hp) {
      hp.forEach((val: number, i: number) => {
        const xx = (i / PTS) * w,
          y = yOf(val);
        g.beginPath();
        g.arc(xx, y, WT.hoverPt === i ? 7 : 5, 0, Math.PI * 2);
        g.fillStyle = WT.hoverPt === i ? css("--human") : "#141414";
        g.fill();
        g.strokeStyle = css("--human");
        g.lineWidth = 2;
        g.stroke();
      });
    }
    g.fillStyle = css("--faint");
    g.font = "10px sans-serif";
    {
      const R2 = rangeOf(d),
        a = audibleHarmonics(R2.lo),
        b = audibleHarmonics(R2.hi),
        n = wtSteps(R2.lo, R2.hi).length;
      g.fillText(tr(`${noteSpan(R2.lo, R2.hi)} で弾いたとき(倍音 ${a === b ? a : `${b}〜${a}`} 本・波形 ${n} 種類)`, `Played on ${noteSpan(R2.lo, R2.hi)} (${a === b ? a : `${b}–${a}`} harmonics · ${n} shape${n > 1 ? "s" : ""})`), 6, h - 5);
    }
  },
  harmLayout(w: number, h: number) {
    return { x0: 30, bw: (w - 34) / HB, top: 18, bot: h - 18 };
  },
  drawHarm() {
    const c = byId<HTMLCanvasElement>("wtHarm");
    if (!c) return;
    const d = shown();
    const { g, w, h } = ctx2d(c);
    const L = WT.harmLayout(w, h);
    const v = WT.view(d);
    if (!v) return;
    // 倍音を描いている間は、描く物(加工の前の倍音)を出す。見るだけのときは鳴る倍音(加工の後)
    const key = d.tool === "harm" ? wtKeyAt(d, d.pos) : null;
    const amps: number[] = key ? key.amps : d.tool === "harm" ? v.pre.amps : v.post.amps;
    const m = Math.max(...amps.slice(0, HB)) || 1;
    // 低い端ではここまで鳴る / 高い端ではここまで
    const R = rangeOf(d),
      kmax = audibleHarmonics(R.lo),
      kHi = audibleHarmonics(R.hi);
    // 目盛り(dB)。低い絵では字が重ならないよう間引く(線は全部引く)
    g.font = "9.5px sans-serif";
    let lastDbY = -1e9;
    [0, -6, -12, -24].forEach((db) => {
      const y = L.bot - Math.pow(10, db / 20) * (L.bot - L.top);
      g.strokeStyle = "#262626";
      g.beginPath();
      g.moveTo(L.x0, y);
      g.lineTo(w, y);
      g.stroke();
      if (y - lastDbY < 11) return;
      lastDbY = y;
      g.fillStyle = css("--faint");
      g.fillText(`${db}`, 2, y + 3);
    });
    for (let k = 0; k < HB; k++) {
      const x = L.x0 + k * L.bw;
      // 棒の置き場(空の枠)。描くときに、どの倍音に描くかが分かるように
      if (d.tool === "harm") {
        g.fillStyle = WT.hoverK === k ? "rgba(93,162,232,0.22)" : "rgba(255,255,255,0.035)";
        g.fillRect(x + 0.5, L.top, Math.max(1, L.bw - 1), L.bot - L.top);
      }
      const val = (amps[k] ?? 0) / m;
      // 間の倍音は「音域の高い鍵盤では鳴らない」で縞
      g.fillStyle = k >= kmax ? "#3a3a3a" : WT.hoverK === k ? css("--human") : k >= kHi ? stripe(g, "#4a8f8a") : d.tool === "harm" ? "#5d8fc8" : "#4a8f8a";
      g.fillRect(x + 0.5, L.bot - val * (L.bot - L.top), Math.max(1, L.bw - 1), val * (L.bot - L.top));
    }
    // 聞こえる上限: 音域の中の段ごとに線(階段)。いちばん高い段より上は「範囲の中の高い鍵盤では鳴らない」帯
    const limX = (k: number) => L.x0 + k * L.bw,
      steps = wtSteps(R.lo, R.hi);
    if (kHi < kmax && kHi < HB) {
      g.fillStyle = "rgba(232,163,58,0.08)";
      g.fillRect(limX(kHi), L.top, Math.min(limX(kmax), L.x0 + HB * L.bw) - limX(kHi), L.bot - L.top);
    }
    let lastTy = L.top - 2;
    [...steps].reverse().forEach((st) => {
      if (st.k >= HB) return;
      const si = steps.indexOf(st),
        x = limX(st.k),
        dim = WT.hiStep != null && WT.hiStep !== si;
      g.globalAlpha = dim ? 0.3 : 1;
      g.strokeStyle = wtStepColor(si);
      g.lineWidth = WT.hiStep === si ? 2 : 1;
      g.setLineDash([3, 3]);
      g.beginPath();
      g.moveTo(x, L.top);
      g.lineTo(x, L.bot);
      g.stroke();
      g.setLineDash([]);
      g.lineWidth = 1;
      const ty = lastTy + 12;
      if (ty > L.bot - 4) {
        g.globalAlpha = 1;
        return;
      }
      lastTy = ty;
      g.fillStyle = wtStepColor(si);
      const t = tr(`${noteSpan(st.from, st.to)} ではここまで`, `up to here on ${noteSpan(st.from, st.to)}`);
      g.fillText(t, clamp(x + 3, 0, w - g.measureText(t).width - 2), ty);
      g.globalAlpha = 1;
    });
    // 番号と周波数
    const f0 = midiHz(R.lo),
      f1 = midiHz(R.hi);
    g.fillStyle = css("--faint");
    axisRow(g, [1, 8, 16, 32, 64, 96].filter((k) => k <= HB).map((k) => [`${k}`, L.x0 + (k - 1) * L.bw] as [string, number]), h - 6, w);
    const info = byId("wtHarmInfo");
    if (info) {
      if (WT.hoverK != null) {
        const k = WT.hoverK,
          db = 20 * Math.log10(Math.max(1e-6, (amps[k] ?? 0) / m));
        // この倍音が鳴らなくなる最初の段
        const cut = steps.find((st) => k >= st.k);
        info.textContent = tr(
          `${k + 1} 番目 · ${hzRange(f0 * (k + 1), f1 * (k + 1))} · ${db.toFixed(1)} dB${k >= kmax ? " · 鳴らない" : cut ? ` · ${noteSpan(cut.from, R.hi)} では鳴らない` : ""}`,
          `#${k + 1} · ${hzRange(f0 * (k + 1), f1 * (k + 1))} · ${db.toFixed(1)} dB${k >= kmax ? " · not played" : cut ? ` · not played on ${noteSpan(cut.from, R.hi)}` : ""}`,
        );
      } else info.textContent = tr("棒にマウスを置くと、番号・周波数・強さ", "Hover a bar for its number, Hz and level");
    }
  },
  /** 鍵盤を押している間、LFO で動く位置を描く(LFO の速さはエンジンの lfo_rate) */
  held: 0,
  anim: 0,
  onKey(_m: number, on: boolean) {
    WT.held = Math.max(0, WT.held + (on ? 1 : -1));
    if (on && WT.held === 1 && !WT.anim) {
      let last = performance.now();
      const tick = () => {
        if (!WT.held || state.inst !== "wavetable") {
          WT.anim = 0;
          WT.draw();
          return;
        }
        const now = performance.now();
        WT.lfoT += ((now - last) / 1000) * 2 * Math.PI * (cur().lfoRate ?? 2);
        last = now;
        WT.draw();
        WT.anim = requestAnimationFrame(tick);
      };
      WT.anim = requestAnimationFrame(tick);
    }
  },
  refresh() {
    rerender();
  },
  help: (): [string, string][] => [
    [tr("作り方", "How it's made"), tr("よくある形・倍音から・録った音から・保存した波形。波形の数(16〜256)", "Common shapes, from harmonics, from recorded audio, saved tables. Frame count (16–256)")],
    [tr("波形の移り変わり", "Frames"), tr("0%(手前)〜 100%(奥)の波形の並び。鳴らすと今の位置が LFO で動く", "Frames from 0% (front) to 100% (back). While playing, the position moves with the LFO")],
    [tr("調整する波形を選ぶ", "Frame to edit"), tr("つまみか、絵・帯のクリックで選ぶ。選んだ波形が白い線になり、下の枠で調整できる", "Pick with the slider or by clicking the picture or strip. The chosen frame turns white and can be edited below")],
    [tr("編集したところ", "Edited frames"), tr("つまみのすぐ下の帯。塗りつぶし = 編集した波形、斜線 = 自動(編集になじませた形)、塗らない = 作り方のまま。クリックでその波形へ(波形は変わらない)", "The strip under the slider. Filled = edited, hatched = auto (blended), empty = as made. Click to go to that frame (it doesn't change)")],
    [tr("なじませる幅", "Blend width"), tr("編集した波形の前後のどこまでを、編集した形へ少しずつ寄せるか。0 ならその波形だけが変わる", "How far around an edited frame to blend toward it. At 0 only that frame changes")],
    [tr("見るだけ", "View"), tr("目のアイコン。調整せずに見る", "The eye icon: just look")],
    [tr("点をつまむ", "Drag points"), tr("波形の欄で ○ をつまんで上下に。太い線が実際に鳴る形", "Drag the ○ up and down in the waveform pane. The thick line is what's played")],
    [tr("線で描く", "Draw"), tr("えんぴつのアイコン。マウスで線を引いて自由に描く。点線 = 描いた形、太い線 = 鳴る形", "The pencil icon: draw freely. Dotted = drawn, thick = played")],
    [tr("倍音を描く", "Draw harmonics"), tr("棒のアイコン。棒をドラッグ。マウスの下の倍音の番号・周波数・強さが出る", "The bars icon: drag bars. Shows the harmonic number, Hz and level under the mouse")],
    [tr("〜で弾いたときを表示", "When played on…"), tr("絵と Hz を、どの音域で弾いたときとして描くか(音色は変わらない)。高い音域ほど出せる倍音が減り、波形が丸くなる(灰色の棒は聞こえない)", "Which range the pictures and Hz assume (the sound doesn't change). Higher ranges allow fewer harmonics, so the shape gets rounder (gray bars aren't heard)")],
    [tr("加工の手順", "Processing"), tr("上から順に掛かる。並べ替え・入り切り・つまみ。AI の加工も同じ所に", "Applied top to bottom. Reorder, toggle, tweak. The AI's steps appear here too")],
    [tr("保存", "Save"), tr("全部の曲で使えるように残す。WAV に書き出してほかのシンセでも", "Keep it for every song. Export WAV for other synths")],
  ],
};

register(WT);
export default WT;
