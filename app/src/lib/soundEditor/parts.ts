// 音色エディタの共通の部品(どの楽器でも同じ見せ方): つまみ・道具の切り替え・欄・凡例・音量の変わり方・フィルタ・
// 音域の選択・鳴っている音・保存。試作(66 版)の部品をそのまま移した
import * as api from "../api";
import { isEn, tr } from "../i18n.svelte";
import { showToast } from "../toast.svelte";
import {
  canEdit,
  cur,
  draw as drawAll,
  ed,
  pushHist,
  rerender,
  setTool,
  shown,
  state,
  toast,
  toolOf,
  ui,
  type Data,
} from "./core.svelte";
import { axisRow, byId, clamp, css, ctx2d, el, fmtS, ico, noteName, tag } from "./dom";

// ---- つまみ ----
/** つまみの名前 → アイコン(何を変えるつまみか)。名前が一致しないものは付けない */
export const KNOB_ICONS: Record<string, string> = {
  倍音の数: "count", 明るさの傾き: "tilt", "dB/oct": "tilt", "奇数・偶数": "oddeven", 奇数: "oddeven", 偶数: "oddeven", フォルマント: "fpos", その強さ: "fheight", その幅: "fwidth",
  ゲイン: "gain", 高域の減衰: "damp", 非調和: "inharm", 揺らぎ: "wobble",
  立ち上がり: "env_a", 下がる時間: "env_d", 保つ音量: "env_s", 余韻: "env_r",
  音量: "level", 強さで明るさ: "velbright",
  長さ: "env_r", 左右: "stereo", 強さの効き方: "velbright", 叩く強さ: "level",
  録音どおりの鍵盤: "note", "区分 1 の鍵盤": "note", 高さの倍率: "ratio", 自分を揺らす: "loop", 変調の強さ: "level", 粒の数: "density",
  元のテンポ: "tempo", カットオフ: "lowpass", レゾナンス: "reso", 強さで開く: "velbright", 時間で開く: "env_a", つなぎ目: "xfade",
  取り出す位置: "position", 進む速さ: "scan", 左右の広がり: "stereo", 位置のばらつき: "spread", 粒の長さ: "w_hann", 音程のばらつき: "pitchrand",
  スナップ: "snap", パンチ: "attack", 音程: "pitchrand", 近いマイク: "mic", 上のマイク: "micup", 部屋のマイク: "room", 強さ: "level",
  から: "select", まで: "select", dB: "gain", 番目まで: "lowpass", 量: "smooth", そろえ方: "phase", 使う範囲: "select",
};
/** つまみの英語の名前(言語が英語のとき) */
export const KNOB_EN: Record<string, string> = {
  倍音の数: "Partials", 明るさの傾き: "Tilt", "奇数・偶数": "Odd / even", 奇数: "Odd", 偶数: "Even", フォルマント: "Formant", その強さ: "Its gain", その幅: "Its width",
  ゲイン: "Gain", 高域の減衰: "High damping", 非調和: "Inharmonicity", 揺らぎ: "Wobble",
  立ち上がり: "Attack", 下がる時間: "Decay", 保つ音量: "Sustain", 余韻: "Release",
  音量: "Level", 強さで明るさ: "Vel → bright", 長さ: "Length", 左右: "Pan", 強さの効き方: "Vel tracking", 叩く強さ: "Hit velocity",
  録音どおりの鍵盤: "Root key", "区分 1 の鍵盤": "Slice 1 key", 高さの倍率: "Ratio", 自分を揺らす: "Feedback", 変調の強さ: "Mod depth", 粒の数: "Density",
  元のテンポ: "Original BPM", カットオフ: "Cutoff", レゾナンス: "Resonance", 強さで開く: "Vel → cutoff", 時間で開く: "Env → cutoff", つなぎ目: "Crossfade",
  取り出す位置: "Position", 進む速さ: "Scan", 左右の広がり: "Spread", 位置のばらつき: "Spray", 粒の長さ: "Grain length", 音程のばらつき: "Pitch rand",
  スナップ: "Snap", パンチ: "Punch", 音程: "Tune", 近いマイク: "Close mic", 上のマイク: "Overhead mic", 部屋のマイク: "Room mic", 強さ: "Amount",
  から: "From", まで: "To", 番目まで: "Up to", 量: "Amount", そろえ方: "Alignment", 使う範囲: "Range", 半音: "Semitones", 倍率: "Ratio",
};
/** やさしい名前にしたつまみの、元の名前と意味(マウスを置くと出る) */
export const KNOB_TIPS: Record<string, () => string> = {
  立ち上がり: () => tr("アタック: 押してから、いちばん大きい音量になるまでの時間", "Attack: time from the key press to the loudest level"),
  下がる時間: () => tr("ディケイ: いちばん大きい所から、保つ音量まで下がる時間", "Decay: time to fall from the peak to the sustain level"),
  保つ音量: () => tr("サステイン: 押している間に保つ音量", "Sustain: level held while the key is down"),
  余韻: () => tr("リリース: 鍵盤を離してから、音が消えるまでの時間", "Release: time for the sound to fade after the key is released"),
  そろえ方: () => tr("位相のそろえ方: 0 = そろえる・1 = 波形どうしでそろえる・2 = ばらす", "Phase alignment: 0 = zero, 1 = align between frames, 2 = randomize"),
  録音どおりの鍵盤: () => tr("元の高さ(ルート音程): この鍵盤で弾くと録音どおりの速さ・高さで鳴る。ほかの鍵盤は半音の差の分だけ速さを変える", "Root key: playing this key plays the recording at its own speed and pitch. Other keys change the speed by the semitone difference"),
  "区分 1 の鍵盤": () => tr("元の高さ(ルート音程): 区分 1 の鍵盤。区分 2 からは半音ずつ上へ並ぶ", "Root key: the key of slice 1. Slice 2 onward go up a semitone each"),
  高さの倍率: () => tr("周波数比: 鍵盤の高さの何倍で鳴らすか。整数倍は楽器らしく、非整数倍は鐘・金属の音", "Frequency ratio: how many times the key's pitch. Whole numbers sound instrument-like; others sound like bells or metal"),
  自分を揺らす: () => tr("フィードバック: 4 番(変調 4 / 出力 4)が自分の音色を自分で変える量(上げるとざらつく)", "Feedback: how much #4 (Mod 4 / Out 4) modulates itself (higher = grittier)"),
  粒の数: () => tr("密度: 1 秒に出す粒の数", "Density: grains per second"),
  取り出す位置: () => tr("ポジション: 粒を切り出す所(素材の頭 = 0%、終わり = 100%)。上の白い線", "Position: where grains are cut from (start of the source = 0%, end = 100%). The white line above"),
  進む速さ: () =>
    tr(
      "スキャン: 押さえている間に、取り出す位置が進む速さ。×1 = 素材の元の速さ、×0.1 = 10 倍に引き伸ばし、− は逆向き(粒そのものは順向きに鳴る)、0 は止まったまま。端を越えたら反対の端へ回る",
      "Scan: how fast the position moves while the key is held. ×1 = the source's own speed, ×0.1 = stretched 10×, negative = backwards (each grain still plays forward), 0 = frozen. Wraps around at the ends",
    ),
  左右の広がり: () => tr("スプレッド(パン): 粒を左右のどこに置くかの幅。0 = 全部真ん中、1 = 左右いっぱい", "Spread (pan): how widely grains are placed left and right. 0 = all centered, 1 = full width"),
  カットオフ: () => tr("ここより上(ローパスのとき)の音を削り始める周波数", "Frequency where the filter starts cutting (above it, for a low-pass)"),
  レゾナンス: () => tr("カットオフの所を持ち上げる量(上げるとクセのある音)", "How much the cutoff point is boosted (higher = more character)"),
};
export const knobLabel = (label: string) => (isEn() ? (KNOB_EN[label] ?? label) : label);

export interface KnobOpts {
  min: number;
  max: number;
  step?: number;
  value: number;
  unit?: string;
  fmt?: (v: number) => string;
  onInput?: (v: number) => void;
  onCommit?: (v: number) => void;
  title?: string;
  cls?: string;
  bind?: string;
  icon?: string;
}
/** つまみ(横のスライダー)。onInput はドラッグ中(試聴だけ)、onCommit は離したとき(履歴に 1 件)。
 *  label は日本語の名前(アイコン・説明・英語の名前を引く鍵) */
export function knob(label: string, { min, max, step = 0.01, value, unit = "", fmt, onInput, onCommit, title, cls = "", bind, icon }: KnobOpts): HTMLLabelElement {
  const out = el("output");
  const show = (v: number) => (out.textContent = (fmt ? fmt(v) : Number(v).toFixed(step >= 1 ? 0 : step >= 0.1 ? 1 : 2)) + unit);
  const inp = el("input", { type: "range", min, max, step, value });
  // 同じ値を 2 か所に出しているつまみ(bind が同じ)は、片方を動かしたらもう片方も合わせる
  const sync = (v: number) => {
    if (bind)
      document.querySelectorAll(`.kn[data-bind="${CSS.escape(bind)}"]`).forEach((k) => {
        if (k !== lab && (k as any).__set) (k as any).__set(v);
      });
  };
  inp.addEventListener("input", () => {
    show(+inp.value);
    onInput?.(+inp.value);
    sync(+inp.value);
  });
  inp.addEventListener("change", () => onCommit?.(+inp.value));
  show(value);
  const ic = icon ?? KNOB_ICONS[label];
  const lab = el("label", { class: "kn " + cls, title: title ?? KNOB_TIPS[label]?.() ?? "", "data-bind": bind ?? null }, el("span", {}, ic ? ico(ic) : null, knobLabel(label)), inp, out);
  (lab as any).__set = (v: number) => {
    inp.value = String(v);
    show(v);
  };
  return lab;
}

/** つまみ(値は d[key])。動かすと描き直し・音に反映 */
export function knobOf(d: Data, label: string, key: string, min: number, max: number, step: number, unit = "", opts: { fmt?: (v: number) => string; title?: string; cls?: string; after?: () => void } = {}) {
  return knob(label, {
    min,
    max,
    step,
    value: d[key],
    unit,
    fmt: opts.fmt,
    title: opts.title,
    cls: opts.cls,
    bind: `${state.track}:${key}`,
    onInput: (v) => {
      d[key] = v;
      drawAll();
    },
    onCommit: (v) => {
      pushHist(tr(`${label}を ${opts.fmt ? opts.fmt(v) : v}${unit} に`, `${knobLabel(label)} → ${opts.fmt ? opts.fmt(v) : v}${unit}`));
      opts.after?.();
    },
  });
}

// ---- 道具・見出し・凡例・欄 ----
export type ToolItem = [key: string, name: string, tip: string, icon?: string, off?: string];
/** 道具の切り替え(アイコン + 名前。同じ幅で並べる)。使えない物は 5 番目に理由 */
export function toolSeg(items: ToolItem[], current: string, onPick: (k: string) => void) {
  const seg = el("div", { class: "seg" });
  items.forEach(([k, n, tip, icon, off]) =>
    seg.append(
      el(
        "button",
        {
          class: (current === k ? "on" : "") + (off ? " off" : ""),
          title: off || tip,
          "aria-disabled": off ? "true" : null,
          onclick: () => {
            if (!off) onPick(k);
          },
        },
        ico(icon ?? k),
        n,
      ),
    ),
  );
  return el("div", { class: "toolseg" }, seg);
}
export const VIEW_TOOL = (): ToolItem => ["view", tr("見るだけ", "View"), tr("見るだけ(調整しない)", "View only (no editing)")];
export const POINT_TOOL = (): ToolItem => ["points", tr("点をつまむ", "Drag points"), tr("点(○)をつまんで動かす", "Drag the points (○)")];

/** 調整の枠の見出し: 「▌◯◯を調整」(白い印 = 上の絵で白く囲んだ物)+ 札 + 右側の部品 */
export function editHead(title: string, pill: string | null, pillCls: string | null, ...right: (Node | null)[]) {
  return el(
    "div",
    { class: "edithead" },
    el("span", { class: "nowmark", title: tr("上の絵で白く囲んだもの", "The one outlined in white above") }),
    el("b", {}, title),
    pill ? el("span", { class: "pill " + (pillCls ?? "") }, pill) : null,
    el("span", { class: "spacer" }),
    ...right,
  );
}
export type LegendItem = [swatch: string | HTMLElement, name: string, tip?: string];
/** 凡例の行。items: [見本(要素かクラス名), 名前, 説明?] */
export function legendRow(items: LegendItem[], ...extra: (Node | null)[]) {
  return el(
    "div",
    { class: "legendrow" },
    ...items.map(([sw, n, tip]) => el("span", { title: tip ?? "" }, typeof sw === "string" ? el("span", { class: "sw " + sw }) : sw, n)),
    ...extra,
  );
}
/** 状態で中身が変わる凡例: id を付けて 2 行分の高さを取っておき、描き直しのたびに refreshLegend で合わせる */
export function liveLegend(id: string, items: LegendItem[]) {
  const r = legendRow(items);
  r.id = id;
  r.style.minHeight = "2lh";
  r.style.alignContent = "flex-start";
  r.dataset.sig = items.map((x) => x[1]).join("|");
  return r;
}
export function refreshLegend(id: string, items: LegendItem[]) {
  const old = byId(id);
  if (!old) return;
  const sig = items.map((x) => x[1]).join("|");
  if (old.dataset.sig === sig) return;
  old.replaceWith(liveLegend(id, items));
}
/** 欄(見出し・道具・絵)。絵は ui.panes に登録して、まとめて描き直す */
export function pane(title: string | Node, { info, tools, canvas, below, head, cls = "" }: { info?: HTMLElement; tools?: Node | null; canvas?: Node | null; below?: Node | null; head?: Node | null; cls?: string } = {}) {
  if (info) info.classList.add("paneinfo");
  return el(
    "div",
    { class: "pane " + cls },
    el("div", { class: "panehead" }, el("span", {}, title), head ? el("span", { class: "spacer" }) : null, head ?? null),
    tools ?? null,
    canvas ?? null,
    info ?? null,
    below ?? null,
  );
}

// ---- 鳴っている音(エンジンの出口の周波数・音量。曲を流していれば曲全体) ----
export function specBox(style: string) {
  return el(
    "div",
    { class: "box", style },
    el("h3", {}, tr("鳴っている音", "Now sounding"), el("span", { class: "spacer" }), el("span", { class: "hint", title: tr("曲を流していれば曲全体の周波数", "The whole song's spectrum while it plays") }, tr("周波数", "Spectrum"))),
    el("canvas", { id: "seSpec", class: "fill" }),
    el("div", { class: "row small" }, el("span", { class: "dim" }, tr("音量", "Level")), el("div", { class: "meter" }, el("div", { id: "seMeter" }))),
  );
}
/** 鳴っている音の絵を描く。bands = 帯の真ん中の周波数、db = 帯ごとの強さ。active でなければ案内の文 */
export function drawSpectrum(bands: number[], db: number[] | null) {
  const c = byId<HTMLCanvasElement>("seSpec");
  if (!c) return;
  const { g, w, h } = ctx2d(c);
  const meter = byId("seMeter");
  if (!db || !db.length || Math.max(...db) < -90) {
    g.fillStyle = css("--faint");
    g.font = "11px sans-serif";
    g.fillText(tr("鳴らすか曲を流すと、出ている音の", "Play a key or the song to see"), 8, h / 2 - 7);
    g.fillText(tr("周波数がここに出ます", "the spectrum here"), 8, h / 2 + 9);
    if (meter) meter.style.width = "0";
    return;
  }
  const fmin = 30,
    fmax = 16000;
  g.strokeStyle = "#262626";
  g.fillStyle = css("--faint");
  g.font = "9.5px sans-serif";
  const hzX = (hz: number) => (Math.log(hz / fmin) / Math.log(fmax / fmin)) * w;
  [100, 1000, 10000].forEach((hz) => {
    const x = hzX(hz);
    g.beginPath();
    g.moveTo(x, 0);
    g.lineTo(x, h);
    g.stroke();
  });
  axisRow(g, [100, 1000, 10000].map((hz) => [hz >= 1000 ? `${hz / 1000}k Hz` : `${hz} Hz`, hzX(hz) + 2] as [string, number]), h - 3, w);
  g.strokeStyle = css("--accent");
  g.lineWidth = 1.5;
  g.beginPath();
  let first = true;
  bands.forEach((hz, i) => {
    const x = hzX(hz),
      y = h - 14 - clamp((db[i] + 100) / 80, 0, 1) * (h - 22);
    if (first) g.moveTo(x, y);
    else g.lineTo(x, y);
    first = false;
  });
  g.stroke();
  const peak = Math.max(...db);
  if (meter) meter.style.width = `${clamp((peak + 60) / 60, 0, 1) * 100}%`;
}

// ---- 保存(全部の曲で使える)・保存した音色 ----
export function saveBox(def: string, what: string) {
  const name = el("input", { type: "text", placeholder: tr("名前", "Name"), value: def });
  const save = async () => {
    const n = name.value.trim();
    if (!n) return;
    try {
      const r = await api.savePreset(state.track, n, false);
      toast(tr(`「${r.saved}」として保存しました(全部の曲で使えます)`, `Saved as "${r.saved}" (available in every song)`));
    } catch (e) {
      if (String(e).includes("exists") || String(e).includes("既に")) {
        const r = await api.savePreset(state.track, n, true).catch((e2) => showToast("error", String(e2)));
        if (r) toast(tr(`「${r.saved}」を上書きしました`, `Overwrote "${r.saved}"`));
      } else showToast("error", tr(`保存できませんでした: ${e}`, `Couldn't save: ${e}`));
    }
  };
  return el(
    "div",
    { class: "box" },
    el("h3", {}, tr("保存(全部の曲で使える)", "Save (for every song)")),
    el(
      "div",
      { class: "hint" },
      tr(
        `この音色を、ほかの曲でも使えるように残します(${what}も一緒に。読み込めば続きを調整できる)。読み込みは左の「保存した音色」から。`,
        `Keeps this sound for use in other songs (including ${what}; load it to keep editing). Load it from "Saved sounds" on the left.`,
      ),
    ),
    el("div", { class: "row" }, name, el("button", { class: "btn", onclick: save }, tr("保存する", "Save"))),
  );
}
/** 保存した音色の一覧(この音源のものだけ)。選ぶと読み込む(1 回の取り消しで戻る) */
export function shelfList(kind: string) {
  const box = el("div", { class: "shelf" });
  box.append(el("div", { class: "hint" }, tr("読み込み中…", "Loading…")));
  api
    .listPresets()
    .then(({ presets }) => {
      box.innerHTML = "";
      const mine = presets.filter((p) => p.instrument === kind || p.instrument.toLowerCase().startsWith(kind));
      if (!mine.length) box.append(el("div", { class: "hint" }, tr("まだありません(右下の「保存する」で残せます)", "Nothing yet (save one with “Save” at the bottom right)")));
      mine.forEach((p) =>
        box.append(
          el(
            "div",
            {
              class: "shelfitem",
              title: p.description ?? "",
              onclick: async () => {
                if (!canEdit()) return;
                try {
                  await api.loadPreset(state.track, p.name);
                  toast(tr(`「${p.name}」を読み込みました(続きを調整できます)`, `Loaded "${p.name}" (you can keep editing)`));
                } catch (e) {
                  showToast("error", tr(`読み込めませんでした: ${e}`, `Couldn't load: ${e}`));
                }
              },
            },
            p.name,
          ),
        ),
      );
    })
    .catch((e) => {
      box.innerHTML = "";
      box.append(el("div", { class: "hint" }, String(e)));
    });
  return box;
}

// ---- 音量の変わり方 ----
/** 音量の変わり方の目盛りの長さ(秒)。画面だけの設定で、音色には保存しない(トラックごとに覚える) */
const ENV_RANGES = [1, 2, 5, 10];
const envKey = (id: string) => `${state.track}:${id}`;
export const envT = (id: string) => ui.envT[envKey(id)] ?? 5;
/** つまみの上限: 目盛りの長さとエンジンの上限の小さい方 */
export const envMax = (id: string, engineMax: number) => Math.min(engineMax, envT(id));
export interface Env {
  a: number;
  d: number;
  s: number;
  r: number;
}
/** 音量の変わり方の欄: 点をつまんで、立ち上がり・下がる時間と保つ音量・余韻を直せる。
 *  get() は { a, d, s, r }(秒・0〜1)、set(名前, 値) で書く。
 *  横は本当の時間(左 = 押してから、右 = 離してから。どちらも 0〜目盛りの長さ)。短い時間を細かく扱えるよう、間隔は短い所ほど広い */
export function envPane(
  id: string,
  { title = tr("音量の変わり方", "Level over time"), get, set, max = { a: 10, d: 10, r: 10 }, color, note, below }: { title?: string; get: () => Env; set: (k: keyof Env, v: number) => void; max?: { a: number; d: number; r: number }; color?: string; note?: string; below?: Node | null },
) {
  const tool = toolOf(id);
  // 目盛りに収まらない値があれば、収まる長さを選び直す。ただし、自分で短い目盛りを選んだ直後(値はそのまま)は選んだ長さを守る
  const needOf = () => {
    const e = get();
    return Math.max(e.a + e.d, e.r);
  };
  {
    const need = needOf();
    if (need > envT(id) && need !== ui.envPickNeed[envKey(id)]) ui.envT[envKey(id)] = ENV_RANGES.find((t) => t >= need) ?? 10;
  }
  const T = envT(id);
  const c = el("canvas", { id, class: "fill" });
  const info = el("span", { class: "hint" });
  const pick = el("select", {
    class: "trackpick",
    title: tr("横の目盛りの長さ。つまみの上限もこれに合わせる(音色には保存しない)", "Length of the time axis. The knobs' maximum follows it (not saved with the sound)"),
    onchange: (ev: Event) => {
      ui.envT[envKey(id)] = +(ev.target as HTMLSelectElement).value;
      ui.envPickNeed[envKey(id)] = needOf();
      rerender();
    },
  });
  ENV_RANGES.forEach((t) => pick.append(el("option", { value: t, selected: t === T }, tr(`${t} 秒`, `${t} s`))));
  const el0 = pane(title, {
    head: el("span", { class: "row small" }, el("span", { class: "dim" }, tr("目盛り", "Scale")), pick),
    info,
    tools: toolSeg([VIEW_TOOL(), ["points", tr("点をつまむ", "Drag points"), tr("点(○)をつまんで、長さ・高さを変える", "Drag the points (○) to change times and level")]], tool, (t) => setTool(id, t)),
    canvas: c,
    below,
  });
  let drag: number | null = null,
    hover = -1;
  const lim = { a: Math.min(T, max.a), d: Math.min(T, max.d), r: Math.min(T, max.r) };
  const geo = () => {
    const r = c.getBoundingClientRect(),
      e = get();
    const W = r.width - 24,
      Wp = W * 0.62,
      gap = W * 0.06,
      Wr = W * 0.32,
      top = 18,
      bot = r.height - 30;
    const xp = (t: number) => 12 + Wp * Math.sqrt(clamp(t / T, 0, 1)),
      xr0 = 12 + Wp + gap,
      xr = (t: number) => xr0 + Wr * Math.sqrt(clamp(t / T, 0, 1));
    const x1 = xp(e.a),
      x2 = xp(e.a + e.d),
      x3 = xr0,
      x4 = xr(e.r);
    const ys = bot - (bot - top) * e.s;
    return { e, Wp, Wr, xr0, xp, xr, top, bot, x1, x2, x3, x4, ys, pts: [[x1, top], [x2, ys], [x4, bot]] as [number, number][] };
  };
  const NAMES = [tr("立ち上がり", "attack"), tr("下がる時間・保つ音量", "decay · sustain"), tr("余韻", "release")];
  const TICKS = [0.1, 0.5, 1, 2, 5, 10];
  const draw = () => {
    if (!c.isConnected) return;
    const { g, w, h } = ctx2d(c);
    const G = geo();
    g.font = "9.5px sans-serif";
    const ruler = (xof: (t: number) => number, x0: number, x1: number, label: string) => {
      g.fillStyle = "rgba(255,255,255,0.025)";
      g.fillRect(x0, G.top, x1 - x0, G.bot - G.top);
      g.strokeStyle = "#2a2a2a";
      g.fillStyle = css("--faint");
      const ts = [0, ...TICKS.filter((t) => t <= T)];
      ts.forEach((t) => {
        const x = xof(t);
        g.beginPath();
        g.moveTo(x, G.top);
        g.lineTo(x, G.bot + 4);
        g.stroke();
      });
      // くっつく字は間引く
      axisRow(g, ts.map((t) => [t === 0 ? "0" : `${t}`, xof(t) - (t === 0 ? 0 : 4)] as [string, number]), G.bot + 14, x1 + 4, 3);
      g.fillStyle = css("--dim");
      g.fillText(label, x0, h - 3);
    };
    ruler(G.xp, 12, 12 + G.Wp, tr("押してから(秒)", "After press (s)"));
    ruler(G.xr, G.xr0, G.xr0 + G.Wr, tr("離してから", "After release"));
    g.strokeStyle = "#262626";
    [0, 0.5, 1].forEach((v) => {
      const y = G.bot - (G.bot - G.top) * v;
      g.beginPath();
      g.moveTo(12, y);
      g.lineTo(w - 12, y);
      g.stroke();
    });
    g.setLineDash([2, 3]);
    g.strokeStyle = css("--faint");
    g.beginPath();
    g.moveTo(G.x3, G.top - 6);
    g.lineTo(G.x3, G.bot);
    g.stroke();
    g.setLineDash([]);
    g.fillStyle = css("--faint");
    g.fillText(tr("離す", "release"), G.x3 + 3, G.top - 6);
    const col = color ?? css("--accent");
    const path = () => {
      g.beginPath();
      g.moveTo(12, G.bot);
      g.lineTo(G.x1, G.top);
      g.lineTo(G.x2, G.ys);
      g.lineTo(G.x3, G.ys);
      g.lineTo(G.x4, G.bot);
    };
    path();
    g.lineTo(12, G.bot);
    g.fillStyle = col.startsWith("#") ? col + "22" : "rgba(37,189,177,0.12)";
    g.fill();
    path();
    g.strokeStyle = col;
    g.lineWidth = 2;
    g.stroke();
    g.lineWidth = 1;
    if (tool === "points")
      G.pts.forEach(([x, y], i) => {
        g.beginPath();
        g.arc(x, y, hover === i || drag === i ? 7 : 5, 0, Math.PI * 2);
        g.fillStyle = hover === i || drag === i ? css("--human") : "#141414";
        g.fill();
        g.strokeStyle = css("--human");
        g.lineWidth = 2;
        g.stroke();
        g.lineWidth = 1;
      });
    // 目盛りの外の値: 端に寄せて「→ 8.0 秒」
    const e = G.e;
    if (e.a + e.d > T) tag(g, tr(`→ ${(e.a + e.d).toFixed(1)} 秒`, `→ ${(e.a + e.d).toFixed(1)} s`), G.x2 - 60, G.ys - 8, w, css("--warn"));
    if (e.r > T) tag(g, tr(`→ ${e.r.toFixed(1)} 秒`, `→ ${e.r.toFixed(1)} s`), G.x4 - 60, G.bot - 8, w, css("--warn"));
    const k = drag ?? hover;
    const A = `${knobLabel("立ち上がり")} ${fmtS(e.a)}`,
      D = `${knobLabel("下がる時間")} ${fmtS(e.d)}`,
      S = `${knobLabel("保つ音量")} ${Math.round(e.s * 100)}%`,
      R = `${knobLabel("余韻")} ${fmtS(e.r)}`;
    info.textContent = k != null && k >= 0 ? [A, `${D} · ${S}`, R][k] : `${A} · ${D} · ${S} · ${R}${note ? tr(`(${note})`, ` (${note})`) : ""}`;
  };
  const at = (ev: PointerEvent) => {
    const r = c.getBoundingClientRect();
    return [ev.clientX - r.left, ev.clientY - r.top];
  };
  c.addEventListener("pointerdown", (ev) => {
    if (tool !== "points" || !canEdit()) return;
    const [x, y] = at(ev);
    const i = geo().pts.findIndex(([px, py]) => Math.hypot(px - x, py - y) < 14);
    if (i < 0) return;
    drag = i;
    c.setPointerCapture(ev.pointerId);
  });
  c.addEventListener("pointermove", (ev) => {
    const [x, y] = at(ev);
    const G = geo();
    if (drag == null) {
      hover = tool === "points" ? G.pts.findIndex(([px, py]) => Math.hypot(px - x, py - y) < 14) : -1;
      c.style.cursor = hover >= 0 ? "grab" : "default";
      draw();
      return;
    }
    c.style.cursor = "grabbing";
    const tp = T * Math.pow(clamp((x - 12) / G.Wp, 0, 1), 2),
      trel = T * Math.pow(clamp((x - G.xr0) / G.Wr, 0, 1), 2);
    if (drag === 0) set("a", clamp(tp, 0.001, lim.a));
    else if (drag === 1) {
      set("d", clamp(tp - G.e.a, 0.01, lim.d));
      set("s", clamp((G.bot - y) / (G.bot - G.top), 0, 1));
    } else set("r", clamp(trel, 0.01, lim.r));
    drawAll();
  });
  c.addEventListener("pointerup", () => {
    if (drag == null) return;
    const e = get();
    pushHist(tr(`${title}の${NAMES[drag]}を ${[fmtS(e.a), `${fmtS(e.d)}・${Math.round(e.s * 100)}%`, fmtS(e.r)][drag]} に`, `${title}: ${NAMES[drag]} → ${[fmtS(e.a), `${fmtS(e.d)} · ${Math.round(e.s * 100)}%`, fmtS(e.r)][drag]}`));
    drag = null;
    rerender();
  });
  c.addEventListener("pointerleave", () => {
    hover = -1;
    draw();
  });
  ui.panes.push({ draw });
  return el0;
}

// ---- フィルタ ----
export interface Filter {
  type: string;
  cutoff: number;
  res: number;
}
/** フィルタの欄: 曲線の山(○)をつまんで、カットオフ(横)とレゾナンス(縦)を直せる */
export function filterPane(id: string, { get, set, below }: { get: () => Filter; set: (k: keyof Filter, v: any) => void; below?: Node | null }) {
  const tool = toolOf(id);
  const c = el("canvas", { id, class: "fill" });
  // 2 行分を取っておく(種類で文の長さが変わっても絵の高さが変わらないように)
  const info = el("span", { class: "hint", style: "min-height: 2lh" });
  // 種類はエンジンと同じ 6 つ(なし・ローパス 12/24 dB・ハイパス・バンドパス・ノッチ)。ローパスの傾きは見出しで選ぶ
  const NAMES: Record<string, string> = {
    off: tr("なし", "Off"),
    lp12: tr("ローパス(ゆるい)", "Low-pass (gentle)"),
    lp24: tr("ローパス(急)", "Low-pass (steep)"),
    hp: tr("ハイパス", "High-pass"),
    bp: tr("バンドパス", "Band-pass"),
    notch: tr("ノッチ", "Notch"),
  };
  const pick = (t: string) => {
    if (!canEdit()) return;
    set("type", t);
    pushHist(tr(`フィルタの種類を ${NAMES[t]} に`, `Filter type → ${NAMES[t]}`));
    rerender();
  };
  const ty = get().type,
    isLp = ty === "lp12" || ty === "lp24";
  const types = el(
    "div",
    { class: "typerow" },
    el("span", { class: "dim small", title: tr("フィルタの種類", "Filter type") }, tr("種類", "Type")),
    el(
      "div",
      { class: "toolstack" },
      toolSeg(
        [
          ["off", tr("なし", "Off"), tr("フィルタを通さない", "No filter"), "filteroff"],
          ["lp", tr("ローパス", "Low-pass"), tr("高い音を削る", "Cuts the highs"), "lowpass"],
          ["hp", tr("ハイパス", "High-pass"), tr("低い音を削る", "Cuts the lows"), "hipass"],
        ],
        isLp ? "lp" : ty,
        (t) => pick(t === "lp" ? "lp12" : t),
      ),
      toolSeg(
        [
          ["bp", tr("バンドパス", "Band-pass"), tr("真ん中だけ残す", "Keeps only the middle"), "bandpass"],
          ["notch", tr("ノッチ", "Notch"), tr("カットオフの所だけ削る", "Cuts only around the cutoff"), "notch"],
        ],
        ty,
        pick,
      ),
    ),
  );
  // 削り方はローパスのときだけ。ほかの種類でも場所は取っておく(見出しの高さが変わると、下の絵がずれる)
  const slope = el(
    "span",
    { class: "row small", style: isLp ? "" : "visibility: hidden", "aria-hidden": isLp ? null : "true" },
    el("span", { class: "dim" }, tr("削り方", "Slope")),
    toolSeg(
      [
        ["lp12", tr("ゆるい", "Gentle"), tr("1 オクターブで 12 dB 下がる", "12 dB per octave"), "lowpass"],
        ["lp24", tr("急", "Steep"), tr("1 オクターブで 24 dB 下がる", "24 dB per octave"), "lowpass24"],
      ],
      isLp ? ty : "",
      pick,
    ),
  );
  const el0 = pane(tr("フィルタ", "Filter"), {
    head: slope,
    info,
    tools: el("div", { class: "toolstack" }, toolSeg([VIEW_TOOL(), ["points", tr("点をつまむ", "Drag points"), tr("山(○)をつまむ: 横 = カットオフ、縦 = レゾナンス", "Drag the peak (○): sideways = cutoff, up/down = resonance")]], tool, (t) => setTool(id, t)), types),
    canvas: c,
    below,
  });
  const xOf = (hz: number, w: number) => 6 + (Math.log(hz / 20) / Math.log(1000)) * (w - 12);
  const hzOf = (x: number, w: number) => 20 * Math.pow(1000, clamp((x - 6) / (w - 12), 0, 1));
  const resp = (f: Filter, hz: number) => {
    const r = hz / f.cutoff;
    if (f.type === "off") return 1;
    if (f.type === "notch") return 1 - 0.995 * Math.exp(-Math.pow(Math.log2(r) * (3 + f.res * 8), 2));
    const m = f.type === "lp12" ? 1 / Math.sqrt(1 + Math.pow(r, 4)) : f.type === "lp24" ? 1 / Math.sqrt(1 + Math.pow(r, 8)) : f.type === "hp" ? 1 / Math.sqrt(1 + Math.pow(1 / r, 4)) : 1 / (1 + Math.abs(Math.log2(r)) * 2);
    return m * (1 + f.res * 3 * Math.exp(-Math.pow(Math.log2(r) * 3, 2)));
  };
  let drag = false,
    hover = false;
  const yOf = (m: number, h: number) => {
    const db = 20 * Math.log10(Math.max(m, 1e-4));
    return h - 14 - clamp((db + 36) / 48, 0, 1) * (h - 26);
  };
  const draw = () => {
    if (!c.isConnected) return;
    const { g, w, h } = ctx2d(c);
    const f = get();
    g.strokeStyle = "#262626";
    g.fillStyle = css("--faint");
    g.font = "9.5px sans-serif";
    [100, 1000, 10000].forEach((hz) => {
      const x = xOf(hz, w);
      g.beginPath();
      g.moveTo(x, 0);
      g.lineTo(x, h);
      g.stroke();
    });
    axisRow(g, [100, 1000, 10000].map((hz) => [hz >= 1000 ? `${hz / 1000}k Hz` : `${hz} Hz`, xOf(hz, w) + 2] as [string, number]), h - 3, w);
    // 絵が低いときは、目盛りの字が重ならないよう間を飛ばす(線は全部引く)
    let lastY = -Infinity;
    [12, 0, -12, -24].forEach((db) => {
      const y = yOf(Math.pow(10, db / 20), h);
      g.strokeStyle = "#262626";
      if (db === 0) g.setLineDash([2, 3]);
      g.beginPath();
      g.moveTo(0, y);
      g.lineTo(w, y);
      g.stroke();
      g.setLineDash([]);
      if (y - lastY < 12) return;
      lastY = y;
      g.fillStyle = css("--faint");
      g.fillText(`${db > 0 ? "+" : ""}${db} dB`, 2, y - 2);
    });
    g.strokeStyle = css("--accent");
    g.lineWidth = 2;
    g.beginPath();
    for (let px = 0; px < w; px++) {
      const y = yOf(resp(f, hzOf(px, w)), h);
      if (px) g.lineTo(px, y);
      else g.moveTo(px, y);
    }
    g.stroke();
    if (tool === "points" && f.type !== "off") {
      const x = xOf(f.cutoff, w),
        y = yOf(resp(f, f.cutoff), h);
      g.beginPath();
      g.arc(x, y, hover || drag ? 7 : 5, 0, Math.PI * 2);
      g.fillStyle = hover || drag ? css("--human") : "#141414";
      g.fill();
      g.strokeStyle = css("--human");
      g.lineWidth = 2;
      g.stroke();
    }
    info.textContent =
      f.type === "off"
        ? tr("なし(フィルタを通さない)", "Off (no filter)")
        : `${NAMES[f.type]} · ${f.cutoff >= 1000 ? (f.cutoff / 1000).toFixed(1) + "k" : Math.round(f.cutoff)} Hz · ${f.type === "notch" ? tr("細さ", "Narrowness") : knobLabel("レゾナンス")} ${Math.round(f.res * 100)}%`;
  };
  c.addEventListener("pointerdown", (ev) => {
    if (tool !== "points" || !canEdit()) return;
    if (get().type === "off") return toast(tr("「なし」のときは、つまむ山がありません(種類を選んでください)", "With “Off” there's no peak to drag (pick a type)"));
    drag = true;
    c.setPointerCapture(ev.pointerId);
  });
  c.addEventListener("pointermove", (ev) => {
    const r = c.getBoundingClientRect(),
      x = ev.clientX - r.left,
      y = ev.clientY - r.top,
      f = get();
    if (!drag) {
      hover = tool === "points" && Math.hypot(x - xOf(f.cutoff, r.width), y - yOf(resp(f, f.cutoff), r.height)) < 16;
      c.style.cursor = tool === "points" ? (hover ? "grab" : "crosshair") : "default";
      draw();
      return;
    }
    c.style.cursor = "grabbing";
    set("cutoff", Math.round(clamp(hzOf(x, r.width), 40, 18000)));
    // 山の高さ(dB)→ レゾナンス(ノッチは谷の細さ: 上ほど細い)
    const db = ((r.height - 14 - y) / (r.height - 26)) * 48 - 36;
    set("res", f.type === "notch" ? clamp(y - 14 < 0 ? 0.95 : 0.95 * (1 - (y - 14) / (r.height - 26)), 0, 0.95) : clamp((Math.pow(10, db / 20) / (f.type === "bp" ? 1 : 0.707) - 1) / 3, 0, 0.95));
    drawAll();
  });
  c.addEventListener("pointerup", () => {
    if (!drag) return;
    drag = false;
    const f = get();
    pushHist(tr(`フィルタを ${Math.round(f.cutoff)} Hz・${f.type === "notch" ? "細さ" : "レゾナンス"} ${Math.round(f.res * 100)}% に`, `Filter → ${Math.round(f.cutoff)} Hz · ${f.type === "notch" ? "narrowness" : "resonance"} ${Math.round(f.res * 100)}%`));
    rerender();
  });
  c.addEventListener("pointerleave", () => {
    hover = false;
    draw();
  });
  ui.panes.push({ draw });
  return el0;
}

// ---- 音域: 絵と Hz を、どの範囲の鍵盤で弾いたときとして描くか(音色は変わらない)。始まり = その C、終わり = その B ----
/** 選んでいる音域(選択欄の値) */
export const rangeSel = (d: Data) => {
  const lo = d.previewNote ?? 60;
  return { lo, hi: Math.max(lo + 11, d.previewHi ?? lo + 11) };
};
/** 絵で描く音域。「C4 だけ」のときは低い端の 1 音として描く */
export const rangeOf = (d: Data) => {
  const r = rangeSel(d);
  return d.onlyLo ? { lo: r.lo, hi: r.lo } : r;
};
export function pitchPick(d: Data, onChange: () => void) {
  const tip = tr(
    "絵と Hz を、この音域で弾いたときとして描く。音色は変わらない(どの鍵盤も同じ形で鳴り、高い音域ほど上の倍音が鳴らなくなる)。広く取ると、範囲の低い端と高い端の両方を描く",
    "Draws the pictures and Hz as if played in this range. The sound doesn't change (every key has the same shape; higher ranges lose upper harmonics). A wide range draws both its low and high ends",
  );
  const R = rangeSel(d),
    octs = [2, 3, 4, 5, 6];
  const a = el("select", {
    class: "trackpick",
    title: tip,
    onchange: (e: Event) => {
      d.previewNote = +(e.target as HTMLSelectElement).value;
      if (rangeSel(d).hi < d.previewNote + 11) d.previewHi = d.previewNote + 11;
      onChange();
    },
  });
  const b = el("select", {
    class: "trackpick",
    title: tip,
    onchange: (e: Event) => {
      d.previewHi = +(e.target as HTMLSelectElement).value;
      if (d.previewHi < (d.previewNote ?? 60) + 11) d.previewNote = d.previewHi - 11;
      onChange();
    },
  });
  octs.forEach((o) => {
    a.append(el("option", { value: 12 * (o + 1), selected: R.lo === 12 * (o + 1) }, `C${o}`));
    b.append(el("option", { value: 12 * (o + 1) + 11, selected: R.hi === 12 * (o + 1) + 11 }, `B${o}`));
  });
  // 表示の切り替え: 範囲の両端(と途中の段)を描くか、低い端の 1 音だけを描くか(画面だけの設定。音色は変わらない)
  const view = el("span", { class: "seg rp-view" });
  (
    [
      [false, `${noteName(R.lo)}〜${noteName(R.hi)}`, tr("範囲の低い端と高い端(ウェーブテーブルは途中の段も)を描く", "Draw the low and high ends of the range (wavetable: the steps between too)")],
      [true, tr(`${noteName(R.lo)} だけ`, `${noteName(R.lo)} only`), tr(`${noteName(R.lo)} で弾いたときだけを描く`, `Draw only ${noteName(R.lo)}`)],
    ] as [boolean, string, string][]
  ).forEach(([v, n, t]) =>
    view.append(
      el(
        "button",
        {
          class: !!d.onlyLo === v ? "on" : "",
          title: t,
          onclick: () => {
            d.onlyLo = v;
            onChange();
          },
        },
        n,
      ),
    ),
  );
  return el(
    "span",
    { class: "row small rangepick", title: tip },
    a,
    el("span", { class: "dim" }, "〜"),
    b,
    el("span", { class: "dim rp-say" }, tr("で弾いたとき", " when played"), el("span", { class: "rp-tail" }, tr("を表示", ""))),
    view,
  );
}

/** 「仕組み」などの説明の吹き出し(マウスを置くと開く)。fill(pop) で中身を作る */
export function explainOn(btn: HTMLElement, cls: string, fill: (pop: HTMLElement) => void) {
  const host = btn.closest(".sound-editor") ?? document.body;
  host.querySelectorAll(`.explain.${cls}`).forEach((e) => e.remove());
  const pop = el("div", { class: `explain ${cls}` });
  host.append(pop);
  btn.addEventListener("mouseenter", () => {
    pop.innerHTML = "";
    fill(pop);
    pop.classList.add("open");
    const r = btn.getBoundingClientRect();
    pop.style.left = `${clamp(r.left - 200, 8, window.innerWidth - 360)}px`;
    const below = r.bottom + 6;
    pop.style.top = `${below}px`;
    requestAnimationFrame(() => {
      const ph = pop.getBoundingClientRect().height;
      if (below + ph > window.innerHeight - 8) pop.style.top = `${Math.max(8, r.top - ph - 6)}px`;
    });
  });
  btn.addEventListener("mouseleave", () => pop.classList.remove("open"));
}

// 楽器の画面から使う物(まとめて引けるように)
export { canEdit, cur, ed, pushHist, rerender, setTool, shown, state, toast, toolOf, ui };
