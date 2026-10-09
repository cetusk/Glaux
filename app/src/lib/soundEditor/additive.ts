// 音色エディタ: 加算合成(正弦波の部分音を 1 本ずつ足す)。
// 左 = 始めの形、上 = 鳴らしてからの変わり方(倍音ごとの消え方・位置のずれ)、
// 下 = 倍音のバランス(棒を描く・山を動かす・描いた山を消す。手で描いた山はエンジンの partial_edits)と音量の変わり方、右 = つまみ
import { tr } from "../i18n.svelte";
import { draw as drawAll, register, setSel, type Data, type Loaded } from "./core.svelte";
import { axisRow, byId, clamp, css, ctx2d, el, envAt, hzRange, midiHz, noteName, stripe, tag } from "./dom";
import {
  canEdit,
  cur,
  envMax,
  envPane,
  knobOf,
  liveLegend,
  pane,
  pitchPick,
  pushHist,
  rangeOf,
  refreshLegend,
  rerender,
  saveBox,
  setTool,
  shelfList,
  shown,
  specBox,
  state,
  toolOf,
  toolSeg,
  ui,
  VIEW_TOOL,
  type LegendItem,
} from "./parts";

/** 部分音の数の上限(エンジンと同じ) */
const P_MAX = 64;
type Edit = { at: number; db: number; g: number };
const PRESETS = (): [string, string, string, Data][] => [
  ["bell", tr("ベル", "Bell"), tr("高い倍音が早く消え、少しずれる", "Upper partials fade fast and drift sharp"), { partials: 32, tilt: -4, oddEven: 0.1, fHz: 1400, fDb: 9, fW: 0.8, damping: 0.5, inharmonic: 0.15, wobble: 0.2, a: 0.01, d: 1.2, s: 0.3, r: 1.5 }],
  ["organ", tr("オルガン", "Organ"), tr("決まった本数を、ずっと同じ強さで", "A fixed set of partials at a steady level"), { partials: 16, tilt: -2, oddEven: -0.5, fHz: 800, fDb: 0, fW: 1, damping: 0.05, inharmonic: 0, wobble: 0.05, a: 0.005, d: 0.2, s: 1, r: 0.08 }],
  ["vox_a", tr("声「あ」", "Voice “ah”"), tr("800 Hz あたりに山", "A hill around 800 Hz"), { partials: 48, tilt: -6, oddEven: 0, fHz: 800, fDb: 14, fW: 0.6, damping: 0.2, inharmonic: 0, wobble: 0.4, a: 0.08, d: 0.5, s: 0.8, r: 0.4 }],
  ["vox_i", tr("声「い」", "Voice “ee”"), tr("2600 Hz あたりに山", "A hill around 2600 Hz"), { partials: 48, tilt: -7, oddEven: 0, fHz: 2600, fDb: 14, fW: 0.5, damping: 0.2, inharmonic: 0, wobble: 0.4, a: 0.08, d: 0.5, s: 0.8, r: 0.4 }],
  ["glass", tr("ガラス", "Glass"), tr("大きくずれて、すぐ消える", "Strongly stretched, fading quickly"), { partials: 24, tilt: -3, oddEven: 0.3, fHz: 3500, fDb: 10, fW: 0.7, damping: 0.8, inharmonic: 0.5, wobble: 0.1, a: 0.002, d: 0.8, s: 0, r: 1.2 }],
  ["strings", tr("弦", "Strings"), tr("多くの倍音を、ゆっくり揺らす", "Many partials, slowly wavering"), { partials: 64, tilt: -5, oddEven: 0, fHz: 1200, fDb: 6, fW: 1.5, damping: 0.3, inharmonic: 0.02, wobble: 0.5, a: 0.25, d: 0.6, s: 0.9, r: 0.8 }],
];
const r1 = (v: number) => Math.round(v * 10) / 10;
const fmtHz = (x: number) => (x >= 1000 ? (x / 1000).toFixed(1) + "k" : String(Math.round(x)));

const ADD: any = {
  kind: "additive",
  title: () => tr("加算合成", "Additive"),
  load({ params: p }: Loaded): Data {
    const n = (k: string, def: number) => (typeof p[k] === "number" ? p[k] : def);
    // 手で描いた山: 「hz|1800:6,2000:9|4000:-6」
    const raw = String(p.partial_edits ?? "");
    const [mode, ...groups] = raw.split("|");
    const drawTo = mode === "idx" ? "idx" : "hz";
    const edits: Edit[] = [];
    if (mode === "hz" || mode === "idx")
      groups.forEach((g, gi) =>
        g.split(",").forEach((pt) => {
          const [a, b] = pt.split(":").map((x) => parseFloat(x));
          if (Number.isFinite(a) && Number.isFinite(b)) edits.push({ at: a, db: b, g: gi + 1 });
        }),
      );
    return {
      partials: Math.round(n("partials", 32)),
      tilt: n("tilt", -6),
      oddEven: n("odd_even", 0),
      fHz: n("formant_hz", 1000),
      fDb: n("formant_db", 0),
      fW: n("formant_width", 0.5),
      damping: n("damping", 0),
      inharmonic: n("inharmonic", 0),
      wobble: n("wobble", 0),
      a: n("attack", 0.01),
      d: n("decay", 1),
      s: n("sustain", 0.8),
      r: n("release", 0.4),
      gain: n("gain_db", -6),
      drawTo,
      edits,
    };
  },
  uiInit: (): Data => ({ previewNote: 60, showShelf: false, preset: null }),
  params: (d: Data) => ({
    partials: d.partials,
    tilt: d.tilt,
    odd_even: d.oddEven,
    formant_hz: d.fHz,
    formant_db: d.fDb,
    formant_width: d.fW,
    damping: d.damping,
    inharmonic: d.inharmonic,
    wobble: d.wobble,
    attack: d.a,
    decay: d.d,
    sustain: d.s,
    release: d.r,
    gain_db: d.gain,
    partial_edits: ADD.editsString(d),
  }),
  staticParams: { wobble: 0 },
  /** 手で描いた山 → partial_edits。山が無くても「何番目」に切り替えてあれば残す(取り消しで一緒に戻るように) */
  editsString(d: Data) {
    if (!d.edits.length && d.drawTo === "hz") return "";
    const gs = [...ADD.groups(d).values()].map((pts: Edit[]) => pts.map((p) => `${Math.round(p.at)}:${r1(p.db)}`).join(","));
    return [d.drawTo, ...gs].join("|");
  },
  hoverK: null as number | null,
  /** つまみ(傾き・奇数偶数・フォルマント)で作った形。いちばん強い倍音を 1 にそろえる(エンジンと同じ式) */
  base(d: Data): number[] {
    const out: number[] = [];
    const f0 = midiHz(d.previewNote ?? 60),
      b = d.inharmonic * d.inharmonic * 0.002;
    for (let k = 1; k <= P_MAX; k++) {
      if (k > d.partials) {
        out.push(0);
        continue;
      }
      const fk = f0 * k * Math.sqrt(1 + b * k * k);
      let a = Math.pow(k, d.tilt / 6.0206);
      if (k > 1) {
        const even = k % 2 === 0;
        if (even && d.oddEven < 0) a *= 1 + d.oddEven;
        else if (!even && d.oddEven > 0) a *= 1 - d.oddEven;
      }
      if (d.fDb > 0) {
        const oct = Math.log2(fk / Math.max(20, d.fHz)) / Math.max(0.05, d.fW);
        a *= Math.pow(10, (d.fDb / 20) * Math.exp(-0.5 * oct * oct));
      }
      out.push(a);
    }
    const m = Math.max(...out) || 1;
    ADD.lastM = m;
    return out.map((a) => a / m);
  },
  f0: (d: Data) => midiHz(d.previewNote ?? 60),
  freqOf(d: Data, k: number, f0: number) {
    const b = d.inharmonic * d.inharmonic * 0.002;
    return f0 * (k + 1) * Math.sqrt(1 + b * (k + 1) * (k + 1));
  },
  /** 周波数が何番目(0 から、小数)に当たるか(非調和で高い倍音ほど上へずれる分も解く) */
  kOfHz(d: Data, hz: number, f0: number) {
    const b = d.inharmonic * d.inharmonic * 0.002,
      r = hz / f0;
    const n = b > 0 ? Math.sqrt((-1 + Math.sqrt(1 + 4 * b * r * r)) / (2 * b)) : r;
    return n - 1;
  },
  posK(d: Data, p: Edit) {
    return d.drawTo === "idx" ? p.at : ADD.kOfHz(d, p.at, ADD.f0(d));
  },
  groups(d: Data): Map<number, Edit[]> {
    const m = new Map<number, Edit[]>();
    d.edits.forEach((p: Edit) => {
      if (!m.has(p.g)) m.set(p.g, []);
      m.get(p.g)!.push(p);
    });
    m.forEach((a) => a.sort((x, y) => x.at - y.at));
    return m;
  },
  /** 周波数の山 1 つ分の上乗せ: 山の中は周波数(対数)でつなぎ、外側は 2 半音かけて 0 へ(エンジンと同じ) */
  hillOffHz(pts: Edit[], f: number) {
    const lf = Math.log2(f),
      first = pts[0],
      last = pts[pts.length - 1],
      taper = 2 / 12;
    if (lf <= Math.log2(first.at)) {
      const t = (Math.log2(first.at) - lf) / taper;
      return t >= 1 ? 0 : first.db * 0.5 * (1 + Math.cos(Math.PI * t));
    }
    if (lf >= Math.log2(last.at)) {
      const t = (lf - Math.log2(last.at)) / taper;
      return t >= 1 ? 0 : last.db * 0.5 * (1 + Math.cos(Math.PI * t));
    }
    for (let i = 0; i < pts.length - 1; i++)
      if (lf <= Math.log2(pts[i + 1].at)) {
        const t = (lf - Math.log2(pts[i].at)) / Math.max(1e-6, Math.log2(pts[i + 1].at) - Math.log2(pts[i].at));
        return pts[i].db + (pts[i + 1].db - pts[i].db) * t;
      }
    return 0;
  },
  /** k 番目に掛かる上乗せ(dB)。山ごとに足す */
  offset(d: Data, k: number) {
    if (!d.edits.length) return 0;
    if (d.drawTo === "idx") return d.edits.filter((p: Edit) => Math.round(p.at) === k).reduce((a: number, p: Edit) => a + p.db, 0);
    const f = ADD.freqOf(d, k, ADD.f0(d));
    let sum = 0;
    ADD.groups(d).forEach((pts: Edit[]) => (sum += ADD.hillOffHz(pts, f)));
    return sum;
  },
  isEdited: (d: Data, k: number) => Math.abs(ADD.offset(d, k)) > 0.05,
  /** 鳴る形: 元の形に手で描いた上乗せを掛ける */
  amps(d: Data): number[] {
    const b = ADD.base(d);
    return b.map((a: number, k: number) => (k < d.partials ? a * Math.pow(10, ADD.offset(d, k) / 20) : 0));
  },
  /** 性質を切り替える: 今の鳴らす高さで、何番目 ⇔ 何 Hz を読み替える(描き直さずに戻せば元の山) */
  convert(d: Data, to: string) {
    if (d.drawTo === to) return;
    const prev = ADD.editsPrev;
    if (prev && prev.track === state.track && prev.mode === to && prev.after === JSON.stringify(d.edits)) {
      d.edits = prev.edits;
      d.drawTo = to;
      ADD.editsPrev = null;
      return;
    }
    const before = { track: state.track, mode: d.drawTo, edits: d.edits.map((p: Edit) => ({ ...p })) };
    const f0 = ADD.f0(d);
    if (to === "idx") {
      const best = new Map<string, Edit>();
      d.edits.forEach((p: Edit) => {
        const k = clamp(Math.round(ADD.kOfHz(d, p.at, f0)), 0, P_MAX - 1),
          key = `${p.g}:${k}`;
        if (!best.has(key) || Math.abs(p.db) > Math.abs(best.get(key)!.db)) best.set(key, { at: k, db: p.db, g: p.g });
      });
      d.edits = [...best.values()];
    } else d.edits = d.edits.map((p: Edit) => ({ at: clamp(Math.round(ADD.freqOf(d, p.at, f0)), 20, 20000), db: p.db, g: p.g }));
    d.drawTo = to;
    ADD.editsPrev = { ...before, after: JSON.stringify(d.edits) };
  },
  atK(d: Data, p: Edit, k: number) {
    return d.drawTo === "idx" ? Math.round(p.at) === k : Math.abs(p.at - ADD.freqOf(d, k, ADD.f0(d))) <= ADD.f0(d) / 2;
  },
  strokeG: 0,
  /** k 番目に棒を描く。隣り合う山・今描いている山とはつなげて 1 つの山にする */
  paint(d: Data, k: number, db: number) {
    const f0 = ADD.f0(d);
    const replaced = d.edits.filter((p: Edit) => ADD.atK(d, p, k));
    d.edits = d.edits.filter((p: Edit) => !ADD.atK(d, p, k));
    const near = d.edits.filter((p: Edit) => Math.abs(ADD.posK(d, p) - k) <= 1.01).map((p: Edit) => p.g);
    const gs = new Set<number>([...replaced.map((p: Edit) => p.g), ...near]);
    if (d.edits.some((p: Edit) => p.g === ADD.strokeG)) gs.add(ADD.strokeG);
    const target = [...gs].find((g) => g !== ADD.strokeG) ?? ADD.strokeG;
    d.edits.forEach((p: Edit) => {
      if (gs.has(p.g)) p.g = target;
    });
    ADD.strokeG = target;
    const at = d.drawTo === "idx" ? k : clamp(Math.round(ADD.freqOf(d, k, f0)), 20, 20000);
    if (d.drawTo === "hz" && ADD.freqOf(d, k, f0) > 20000) return;
    d.edits.push({ at, db, g: target });
  },
  /** k 番目の棒を消す。山の途中を消したら、左右で別の山に分ける */
  erase(d: Data, k: number) {
    const gone = d.edits.filter((p: Edit) => ADD.atK(d, p, k));
    if (!gone.length) return;
    d.edits = d.edits.filter((p: Edit) => !ADD.atK(d, p, k));
    gone.forEach((q: Edit) => {
      const right = d.edits.filter((p: Edit) => p.g === q.g && ADD.posK(d, p) > k),
        left = d.edits.filter((p: Edit) => p.g === q.g && ADD.posK(d, p) < k);
      if (left.length && right.length) {
        const ng = Date.now() + Math.random();
        right.forEach((p: Edit) => (p.g = ng));
      }
    });
  },
  /** 山ごとのつまみ(◇): いちばん上乗せの大きい点の上 */
  hillHandles(d: Data, G: any, am: number[]) {
    const out: any[] = [];
    ADD.groups(d).forEach((pts: Edit[], g: number) => {
      const peak = pts.reduce((a, p) => (p.db > a.db ? p : a));
      const kf = ADD.posK(d, peak),
        k = Math.round(kf);
      if (k < 0 || k >= d.partials) return;
      const x = G.x0 + (kf + 0.5) * G.bw,
        y = G.bot - Math.min(am[k] ?? 0, 1) * (G.bot - G.top) - 12;
      const ks = pts.map((p) => ADD.posK(d, p));
      out.push({ g, x, y: Math.max(G.top + 4, y), peak, lo: Math.min(...ks), hi: Math.max(...ks), pts });
    });
    return out;
  },
  /** 音域の高い端で弾いたときの棒の高さが、低い端と目に見えて違うか */
  hiDiffers(d: Data) {
    const a = ADD.amps(d),
      b = ADD.amps({ ...d, previewNote: rangeOf(d).hi });
    return a.some((v: number, k: number) => k < d.partials && Math.abs(Math.min(v, 1) - Math.min(b[k], 1)) > 0.03);
  },
  /** 凡例: 今の絵に出ている物だけ */
  legendItems(d: Data): LegendItem[] {
    const tool = toolOf("bars"),
      R = rangeOf(d),
      f0 = midiHz(R.lo),
      f1 = midiHz(R.hi),
      ks = [...Array(d.partials).keys()];
    const drawing = d.edits.length > 0 || ["harm", "move", "erase"].includes(tool);
    const muteLo = ks.some((k) => ADD.freqOf(d, k, f0) >= 20000),
      muteHi = ks.some((k) => ADD.freqOf(d, k, f0) < 20000 && ADD.freqOf(d, k, f1) >= 20000);
    return [
      ["base", tr(`${noteName(R.lo)} のときの強さ`, `Level at ${noteName(R.lo)}`), tr(`${noteName(R.lo)} で弾いたときの倍音の強さ。右の「形」のつまみで決まり、手で描いた山の所は青`, `Partial levels when playing ${noteName(R.lo)}. Set by the “Shape” knobs on the right; hand-drawn hills are blue`)],
      ...(drawing
        ? ([
            ["drawn", tr("手で描いた山", "Hand-drawn hill")],
            ["hzband", tr("手で描いた山の範囲", "Hill extent"), tr("続けて描いた棒のまとまり = 1 つの山。「山を動かす」の ◇ でつかめる", "Bars drawn in one stroke = one hill. Grab it by its ◇ with “Move hills”")],
            [el("span", { class: "sw dash" }), tr("描く前の高さ", "Before drawing")],
          ] as LegendItem[])
        : []),
      ...(muteLo ? ([["mute", tr("聞こえない", "Inaudible"), tr("この音域では 20 kHz を超えて聞こえない", "Above 20 kHz in this range")]] as LegendItem[]) : []),
      ...(muteHi ? ([["mute2", tr("高い側で聞こえない", "Inaudible up high"), tr("音域の高い端の鍵盤では 20 kHz を超えて聞こえない(低い端では鳴る)", "Above 20 kHz at the top of the range (audible at the bottom)")]] as LegendItem[]) : []),
      ...(ADD.hiDiffers(d)
        ? ([
            [
              el("span", { class: "sw", style: "background: transparent; border: 1.5px solid rgba(240,200,120,0.95)" }),
              tr(`${noteName(R.hi)} のときの強さ`, `Level at ${noteName(R.hi)}`),
              tr("音域の高い端で弾いたときの棒の高さ(輪郭)。周波数で決まる山は、高い鍵盤ほど下の番目へ移る", "Bar heights at the top of the range (outline). Frequency-fixed hills move to lower partials on higher keys"),
            ],
          ] as LegendItem[])
        : []),
    ];
  },
  layout(r: { width: number; height: number }) {
    return { x0: 30, bw: (r.width - 34) / P_MAX, top: 22, bot: r.height - 18 };
  },
  render(body: HTMLElement) {
    const d = cur();
    body.innerHTML = "";
    // ---- 左: 始めの形 ----
    const left = el("div", { class: "col left" });
    const srcSeg = el("div", { class: "seg" });
    (
      [
        ["presets", tr("よくある形", "Common shapes")],
        ["shelf", tr("保存した音色", "Saved sounds")],
      ] as [string, string][]
    ).forEach(([k, n]) =>
      srcSeg.append(
        el(
          "button",
          {
            class: (k === "shelf") === !!d.showShelf ? "on" : "",
            onclick: () => {
              d.showShelf = k === "shelf";
              rerender();
            },
          },
          n,
        ),
      ),
    );
    const list = el("div", { class: "shapes" });
    PRESETS().forEach(([k, n, desc, p]) => {
      const c = el("canvas");
      list.append(
        el(
          "div",
          {
            class: "shape" + (d.preset === k ? " on" : ""),
            title: desc,
            onclick: () => {
              if (!canEdit()) return;
              Object.assign(d, p, { preset: k, edits: [] });
              pushHist(tr(`形を「${n}」に`, `Shape → “${n}”`));
              rerender();
            },
          },
          c,
          el("div", {}, n, el("small", {}, desc)),
        ),
      );
      requestAnimationFrame(() => {
        if (!c.isConnected) return;
        const { g, w, h } = ctx2d(c);
        const am = ADD.base({ ...p, previewNote: 60 });
        const bw = w / 32;
        am.slice(0, 32).forEach((a: number, i: number) => {
          g.fillStyle = "#7fd3cc";
          g.fillRect(i * bw, h - a * (h - 2), Math.max(1, bw - 1), a * (h - 2));
        });
      });
    });
    left.append(
      el(
        "div",
        { class: "box" },
        el("h3", {}, tr("始めの形", "Starting shape"), el("span", { class: "spacer" }), el("span", { class: "hint" }, tr("選ぶとつまみが変わる", "Sets the knobs"))),
        srcSeg,
        d.showShelf ? shelfList("additive") : list,
        el("div", { class: "hint" }, tr("選んだ形から、右のつまみと下の「倍音のバランスを調整」で作り込みます。", "Refine it with the knobs on the right and “Adjust the partial balance” below.")),
      ),
    );
    // ---- 真ん中: 上 = 見る(鳴らしてからの変わり方・鳴っている音)/ 下 = 直す(倍音・包絡) ----
    const center = el("div", { class: "col center wtcol", style: "overflow-y: auto" });
    const grid = el("div", { class: "wtgrid", style: "grid-template-rows: minmax(174px, 0.75fr) minmax(300px, 1.6fr)" });
    const tool = toolOf("bars");
    const decay = el("canvas", { id: "addTime", class: "fill" });
    const freq = el("canvas", { id: "addFreq", class: "fill" });
    const tinfo = el("span", { class: "hint", id: "addTimeInfo" }),
      finfo = el("span", { class: "hint", id: "addFreqInfo", style: "min-height: 2lh" });
    grid.append(
      el(
        "div",
        { class: "box", style: "grid-column: span 4" },
        el("h3", {}, tr("鳴らしてからの変わり方", "How it changes after the hit"), el("span", { class: "spacer" }), el("span", { class: "hint" }, tr("白 = 下でマウスを置いている倍音", "White = the partial under the mouse below"))),
        el(
          "div",
          { class: "editpanes" },
          pane(tr("倍音ごとの消え方", "How each partial fades"), { info: tinfo, canvas: decay, below: knobOf(d, "高域の減衰", "damping", 0, 1, 0.01, "", { title: tr("高い倍音ほど早く消える度合い", "How much faster upper partials fade") }) }),
          pane(tr("倍音の位置のずれ", "Partial positions"), { info: finfo, canvas: freq, below: knobOf(d, "非調和", "inharmonic", 0, 1, 0.01, "", { title: tr("高い倍音ほど上へずれる(ピアノ・鐘)", "Upper partials drift sharp (piano, bells)") }) }),
        ),
      ),
    );
    grid.append(specBox("grid-column: span 2"));
    const bars = el("canvas", { id: "addBars", class: "fill" + (tool === "erase" ? " erasing" : "") });
    const binfo = el("span", { class: "hint", id: "addInfo" });
    const nEdits = d.edits.length;
    grid.append(
      el(
        "div",
        { class: "box editgroup", style: "grid-column: span 6" },
        el(
          "div",
          { class: "edithead" },
          el("b", {}, tr("倍音のバランスを調整", "Adjust the partial balance")),
          nEdits
            ? el(
                "button",
                {
                  class: "btn quiet",
                  onclick: () => {
                    if (!canEdit()) return;
                    d.edits = [];
                    pushHist(tr("手で描いた山を全部消す", "Clear all hand-drawn hills"));
                    rerender();
                  },
                },
                tr("手で描いた山を全部消す", "Clear hand-drawn hills"),
              )
            : null,
          el("span", { class: "spacer" }),
          pitchPick(d, () => rerender()),
        ),
        el(
          "div",
          { class: "editpanes wide-left" },
          pane(tr("倍音(1 本 = 1 つ。左ほど低い)", "Partials (one bar each; lower on the left)"), {
            info: binfo,
            tools: el(
              "div",
              { class: "toolstack" },
              toolSeg(
                [
                  VIEW_TOOL(),
                  ["move", tr("山を動かす", "Move hills"), tr("フォルマントの山(◆)や手で描いた山(◇)をつまんで、位置(横)と強さ(縦)を変える", "Drag the formant (◆) or a hand-drawn hill (◇): across = position, up/down = strength"), "move"],
                  ["harm", tr("棒を描く", "Draw bars"), tr("棒を上下にドラッグして、1 本ずつ強さを描く", "Drag over the bars to draw each partial's level"), "harm"],
                  ["erase", tr("描いた山を消す", "Erase hills"), tr("消しゴム。なぞった所の、手で描いた分を消す(元の形に戻る)", "Eraser: removes what was hand-drawn where you drag (back to the base shape)"), "erase"],
                ],
                tool,
                (t) => setTool("bars", t),
              ),
              tool === "harm" || (tool === "move" && nEdits) ? ADD.drawToRow(d) : null,
            ),
            canvas: bars,
            below: liveLegend("addLegend", ADD.legendItems(d)),
          }),
          envPane("addEnv", { get: () => shown() as any, set: (k, v) => (cur()[k] = v), max: { a: 10, d: 10, r: 10 } }),
        ),
      ),
    );
    center.append(grid);
    // ---- 右: つまみ ----
    const right = el("div", { class: "col right" });
    const k = (label: string, key: string, min: number, max: number, step: number, unit = "", opts: any = {}) => knobOf(d, label, key, min, max, step, unit, opts);
    right.append(
      el(
        "div",
        { class: "box" },
        el("h3", {}, tr("形", "Shape")),
        k("倍音の数", "partials", 8, P_MAX, 1),
        k("明るさの傾き", "tilt", -24, 3, 0.5, " dB/oct"),
        k("奇数・偶数", "oddEven", -1, 1, 0.05, "", {
          title: tr("− で偶数を減らす(−1 で奇数だけ = 矩形波のような空洞の音)・+ で奇数を減らす(+1 で基音と偶数 = 明るく開いた音)。基音は残る", "− removes even partials (−1 = odd only, hollow like a square wave); + removes odd ones (+1 = fundamental and evens, open and bright). The fundamental stays"),
        }),
        k("フォルマント", "fHz", 200, 5000, 10, " Hz", { title: tr("山の位置(下の ◆ でも動かせる)。Hz で決まるので、音域によって何番目の倍音に当たるかが変わる", "Hill position (also movable with ◆ below). It's in Hz, so which partial it hits depends on the range") }),
        k("その強さ", "fDb", 0, 24, 0.5, " dB"),
        k("その幅", "fW", 0.1, 2, 0.05, " oct"),
        k("ゲイン", "gain", -24, 6, 0.5, " dB"),
      ),
    );
    right.append(el("div", { class: "box" }, el("h3", {}, tr("動き", "Motion")), k("高域の減衰", "damping", 0, 1, 0.01), k("非調和", "inharmonic", 0, 1, 0.01), k("揺らぎ", "wobble", 0, 1, 0.01, "", { title: tr("倍音ごとの細かな揺れ(生っぽさ)", "Small per-partial wavering (liveliness)") })));
    right.append(
      el(
        "div",
        { class: "box" },
        el("h3", {}, tr("音量の変わり方", "Level over time"), el("span", { class: "spacer" }), el("span", { class: "hint" }, tr("押す → 離す", "press → release"))),
        k("立ち上がり", "a", 0.001, envMax("addEnv", 10), 0.001, " s"),
        k("下がる時間", "d", 0.01, envMax("addEnv", 10), 0.01, " s"),
        k("保つ音量", "s", 0, 1, 0.01),
        k("余韻", "r", 0.01, envMax("addEnv", 10), 0.01, " s"),
      ),
    );
    right.append(saveBox(state.trackName, tr("つまみと手で描いた山", "the knobs and hand-drawn hills")));
    body.append(left, center, right);
    // ---- 倍音の操作 ----
    let drag: string | null = null;
    const L = () => ADD.layout(bars.getBoundingClientRect());
    const partialAt = (ev: PointerEvent) => {
      const r = bars.getBoundingClientRect(),
        G = L();
      const x = ev.clientX - r.left;
      return x < G.x0 ? null : clamp(Math.floor((x - G.x0) / G.bw), 0, P_MAX - 1);
    };
    const nearLine = (ev: PointerEvent) => {
      const r = bars.getBoundingClientRect(),
        G = L();
      return tool !== "view" && Math.abs(ev.clientX - r.left - (G.x0 + d.partials * G.bw)) < 7;
    };
    bars.addEventListener("pointerdown", (ev) => {
      if (tool === "view" || !canEdit()) return;
      if (nearLine(ev)) {
        drag = "count";
        ADD.dragLine = true;
        bars.setPointerCapture(ev.pointerId);
        return;
      }
      const r = bars.getBoundingClientRect(),
        G = L();
      if (tool === "move") {
        const hit = ADD.grabAt(d, G, ev.clientX - r.left, ev.clientY - r.top);
        if (!hit) return;
        if (hit.g != null) {
          ADD.hillDrag = { g: hit.g, x0: ev.clientX, y0: ev.clientY, orig: d.edits.filter((p: Edit) => p.g === hit.g).map((p: Edit) => ({ ...p })), peakK: ADD.posK(d, hit.peak) };
          drag = "hill";
          bars.setPointerCapture(ev.pointerId);
          return;
        }
      }
      if (tool === "harm") ADD.strokeG = Date.now();
      if (tool === "move") ADD.dragFormant = true;
      drag = tool;
      bars.setPointerCapture(ev.pointerId);
      ADD.move(ev, drag);
    });
    bars.addEventListener("pointermove", (ev) => {
      if (drag === "count") {
        const r = bars.getBoundingClientRect(),
          G = L();
        d.partials = clamp(Math.round((ev.clientX - r.left - G.x0) / G.bw), 8, P_MAX);
        drawAll();
        return;
      }
      ADD.hoverLine = !drag && nearLine(ev);
      ADD.hoverK = ADD.hoverLine ? null : partialAt(ev);
      if (ADD.hoverLine) {
        bars.style.cursor = "ew-resize";
        ADD.draw();
        return;
      }
      if (tool === "move") {
        const r = bars.getBoundingClientRect(),
          G = L();
        const hit = drag ? null : ADD.grabAt(d, G, ev.clientX - r.left, ev.clientY - r.top);
        ADD.hoverHill = hit && hit.g != null ? hit.g : null;
        if (!drag) ADD.hoverFormant = !!hit && hit.g == null;
        bars.style.cursor = drag || hit ? "grab" : "default";
      } else bars.style.cursor = tool === "view" ? "default" : tool === "erase" ? "" : "crosshair";
      if (drag === "hill") ADD.moveHill(ev);
      else if (drag) ADD.move(ev, drag);
      else ADD.draw();
    });
    bars.addEventListener("pointerup", () => {
      if (!drag) return;
      ADD.dragLine = false;
      ADD.hillDrag = null;
      ADD.dragFormant = false;
      pushHist(
        drag === "hill"
          ? tr("手で描いた山を動かす", "Move a hand-drawn hill")
          : drag === "count"
            ? tr(`倍音の数を ${d.partials} に`, `Partials → ${d.partials}`)
            : drag === "harm"
              ? tr(`${(ADD.hoverK ?? 0) + 1} 番目あたりの倍音を描く`, `Draw partials around #${(ADD.hoverK ?? 0) + 1}`)
              : drag === "erase"
                ? tr("描いた山を消す", "Erase hand-drawn hills")
                : tr(`フォルマントを ${Math.round(d.fHz)} Hz・${d.fDb.toFixed(1)} dB に`, `Formant → ${Math.round(d.fHz)} Hz, ${d.fDb.toFixed(1)} dB`),
      );
      setSel(drag === "move" ? tr("フォルマント", "Formant") : drag === "hill" ? tr("手で描いた山", "Hand-drawn hill") : tr("倍音", "Partials"));
      drag = null;
      rerender();
    });
    bars.addEventListener("pointerleave", () => {
      ADD.hoverK = null;
      ADD.hoverLine = false;
      ADD.hoverHill = null;
      ADD.hoverFormant = false;
      ADD.draw();
    });
    decay.addEventListener("pointermove", (ev) => {
      const r = decay.getBoundingClientRect();
      ADD.hoverT = [(ev.clientX - r.left) / r.width, 1 - (ev.clientY - r.top) / r.height];
      ADD.draw();
    });
    decay.addEventListener("pointerleave", () => {
      ADD.hoverT = null;
      ADD.draw();
    });
    freq.addEventListener("pointermove", (ev) => {
      const r = freq.getBoundingClientRect();
      ADD.hoverF = (ev.clientX - r.left) / r.width;
      ADD.draw();
    });
    freq.addEventListener("pointerleave", () => {
      ADD.hoverF = null;
      ADD.draw();
    });
  },
  /** 「高さを変えたら、描いた山は」の切り替え。ボタンにマウスを置くと、C3 と C5 で山がどこに来るかを小さな絵で見せる */
  drawToRow(d: Data) {
    const host = (document.querySelector(".sound-editor") ?? document.body) as HTMLElement;
    host.querySelectorAll(".explain.drawto").forEach((e) => e.remove());
    const pop = el("div", { class: "explain drawto" });
    host.append(pop);
    const name = (t: string) => (t === "hz" ? tr("同じ周波数に残る", "Stay at the same Hz") : tr("音と一緒に上下する", "Move with the note"));
    const seg = toolSeg(
      [
        ["hz", name("hz"), "", "pin"],
        ["idx", name("idx"), "", "follow"],
      ],
      d.drawTo ?? "hz",
      (t) => {
        if (!canEdit()) return;
        if (d.edits.length) {
          ADD.convert(d, t);
          pushHist(tr(`手で描いた山を「${name(t)}」に`, `Hand-drawn hills → “${name(t)}”`));
        } else {
          d.drawTo = t;
          pushHist(tr(`これから描く山を「${name(t)}」に`, `New hills → “${name(t)}”`));
        }
        rerender();
      },
    );
    seg.querySelectorAll("button").forEach((b, i) => {
      b.addEventListener("mouseenter", () => {
        ADD.explain(pop, i === 0 ? "hz" : "idx");
        pop.classList.add("open");
        const r = b.getBoundingClientRect(),
          ph = pop.offsetHeight;
        pop.style.left = `${clamp(r.left, 8, window.innerWidth - pop.offsetWidth - 8)}px`;
        pop.style.top = `${r.bottom + 6 + ph < window.innerHeight ? r.bottom + 6 : Math.max(8, r.top - ph - 6)}px`;
      });
      b.addEventListener("mouseleave", () => pop.classList.remove("open"));
    });
    return el("div", { class: "toolrow" }, el("span", { class: "dim small" }, tr("高さを変えたら、描いた山は", "When the pitch changes, hills")), el("div", { style: "flex: 1; min-width: 0" }, seg));
  },
  explain(pop: HTMLElement, mode: string) {
    pop.innerHTML = "";
    const c = el("canvas", { style: "height: 150px; width: 100%" });
    pop.append(
      el("b", {}, mode === "hz" ? tr("同じ周波数に残る", "Stay at the same Hz") : tr("音と一緒に上下する", "Move with the note")),
      c,
      el(
        "div",
        { class: "small" },
        mode === "hz"
          ? tr("2 kHz に描いた山は、どの高さで弾いても 2 kHz。高い音ほど低い番目の倍音に当たる。声の母音・楽器の胴の響きのような山に(◆ の山と同じ動き)。", "A hill drawn at 2 kHz stays at 2 kHz at any pitch, so higher notes hit lower partials. Good for vowel-like or body-resonance hills (like ◆).")
          : tr("8 番目の倍音に描いた山は、どの高さで弾いても 8 番目。音が上がると山の周波数も一緒に上がる。「この倍音だけ強く」のような、倍音の並びそのものに。", "A hill drawn on partial 8 stays on partial 8 at any pitch, so its frequency rises with the note. Good for shaping the harmonic series itself."),
      ),
    );
    requestAnimationFrame(() => {
      const { g, w, h } = ctx2d(c);
      (
        [
          [48, tr("C3 で弾いたとき", "Played at C3")],
          [72, tr("C5 で弾いたとき", "Played at C5")],
        ] as [number, string][]
      ).forEach(([m, label], row) => {
        const f0 = midiHz(m),
          y0 = row * (h / 2) + 14,
          hh = h / 2 - 26,
          K = 24,
          bw = (w - 8) / K;
        g.fillStyle = css("--dim");
        g.font = "10.5px sans-serif";
        g.fillText(label, 4, y0 - 2);
        const kc = mode === "hz" ? 2000 / f0 : 8;
        for (let k = 1; k <= K; k++) {
          const hill = mode === "hz" ? Math.exp(-0.5 * Math.pow(Math.log2((k * f0) / 2000) / 0.2, 2)) : Math.exp(-0.5 * Math.pow((k - kc) / 1.1, 2));
          const a = Math.min(1, (1 / Math.pow(k, 0.7)) * (1 + 2.2 * hill));
          g.fillStyle = hill > 0.3 ? css("--human") : "#4a8f8a";
          g.fillRect(4 + (k - 1) * bw + 1, y0 + hh - a * hh, bw - 2, a * hh);
        }
        const x = 4 + (kc - 0.5) * bw;
        g.strokeStyle = "#fff";
        g.setLineDash([3, 3]);
        g.beginPath();
        g.moveTo(x, y0);
        g.lineTo(x, y0 + hh);
        g.stroke();
        g.setLineDash([]);
        tag(g, mode === "hz" ? tr(`2 kHz = ${kc.toFixed(1)} 番目`, `2 kHz = partial ${kc.toFixed(1)}`) : tr(`8 番目 = ${((f0 * 8) / 1000).toFixed(1)} kHz`, `Partial 8 = ${((f0 * 8) / 1000).toFixed(1)} kHz`), x + 4, y0 + 10, w);
        g.fillStyle = css("--faint");
        g.font = "9.5px sans-serif";
        [1, 8, 16, 24].forEach((k) => g.fillText(String(k), 4 + (k - 1) * bw, y0 + hh + 10));
      });
    });
  },
  /** 山を動かすでつかめる物: ◆(フォルマント)か ◇(手で描いた山)の近い方 */
  grabAt(d: Data, G: any, x: number, y: number) {
    const c: any[] = [{ g: null, dist: Math.hypot(x - ADD.formantX(d, G), y - ADD.formantY(d, G)) }];
    ADD.hillHandles(d, G, ADD.amps(d)).forEach((h: any) => c.push({ g: h.g, peak: h.peak, dist: Math.hypot(x - h.x, y - h.y) }));
    const best = c.sort((a, b) => a.dist - b.dist)[0];
    return best.dist < 16 ? best : null;
  },
  /** ◇ を動かす: 左右 = 山の形のまま位置、上下 = 山全体を同じ dB */
  moveHill(ev: PointerEvent) {
    const d = cur(),
      hd = ADD.hillDrag,
      r = byId("addBars")!.getBoundingClientRect(),
      G = ADD.layout(r);
    const dk = (ev.clientX - hd.x0) / G.bw,
      ddb = (-(ev.clientY - hd.y0) / (G.bot - G.top)) * 40;
    const moved = hd.orig.map((p: Edit) => {
      const at = d.drawTo === "idx" ? p.at + Math.round(dk) : Math.round(p.at * ((hd.peakK + 1 + dk) / (hd.peakK + 1)));
      return { ...p, at, db: clamp(p.db + ddb, -60, 40) };
    });
    const ok = d.drawTo === "idx" ? moved.every((p: Edit) => p.at >= 0 && p.at < P_MAX) : moved.every((p: Edit) => p.at >= 20 && p.at <= 20000);
    if (!ok) return;
    d.edits = [...d.edits.filter((p: Edit) => p.g !== hd.g), ...moved];
    drawAll();
  },
  formantK: (d: Data) => ADD.kOfHz(d, d.fHz, midiHz(d.previewNote ?? 60)) + 1,
  formantX: (d: Data, G: any) => G.x0 + clamp((ADD.formantK(d) - 0.5) * G.bw, 0, P_MAX * G.bw),
  formantY: (d: Data, G: any) => G.bot - (0.1 + (d.fDb / 24) * 0.85) * (G.bot - G.top),
  move(ev: PointerEvent, drag: string) {
    const d = cur(),
      c = byId("addBars")!,
      r = c.getBoundingClientRect(),
      G = ADD.layout(r);
    const x = ev.clientX - r.left,
      y = ev.clientY - r.top;
    const k = clamp(Math.floor((x - G.x0) / G.bw), 0, P_MAX - 1),
      v = clamp((G.bot - y) / (G.bot - G.top), 0, 1);
    if (drag === "harm") {
      if (k < d.partials) {
        // 目標の高さ → 元の形への上乗せ(dB)
        const b = Math.max(ADD.base(d)[k], 1e-4);
        ADD.paint(d, k, clamp(20 * Math.log10(Math.max(v, 1e-3) / b), -60, 40));
      }
    } else if (drag === "erase") ADD.erase(d, k);
    else {
      d.fHz = clamp(Math.round(ADD.freqOf(d, (x - G.x0) / G.bw - 0.5, midiHz(d.previewNote ?? 60))), 200, 5000);
      d.fDb = r1(clamp(((G.bot - y) / (G.bot - G.top) - 0.1) / 0.85 * 24, 0, 24));
    }
    ADD.hoverK = k;
    drawAll();
  },
  draw() {
    const d = shown();
    if (!d) return;
    const c = byId<HTMLCanvasElement>("addBars");
    if (!c) return;
    const { g, w, h } = ctx2d(c);
    const G = ADD.layout({ width: w, height: h });
    const base = ADD.base(d),
      am = ADD.amps(d),
      tool = toolOf("bars");
    const f0 = midiHz(d.previewNote ?? 60),
      Rg = rangeOf(d),
      f1 = midiHz(Rg.hi);
    g.font = "9.5px sans-serif";
    let lastDbY = -1e9;
    [0, -6, -12, -24].forEach((db) => {
      const y = G.bot - Math.pow(10, db / 20) * (G.bot - G.top);
      g.strokeStyle = "#262626";
      g.beginPath();
      g.moveTo(G.x0, y);
      g.lineTo(w, y);
      g.stroke();
      if (y - lastDbY < 11) return;
      lastDbY = y;
      g.fillStyle = css("--faint");
      g.fillText(`${db}`, 2, y + 3);
    });
    for (let k = 0; k < P_MAX; k++) {
      const x = G.x0 + k * G.bw,
        bwid = Math.max(1, G.bw - 2);
      const audible = ADD.freqOf(d, k, f0) < 20000,
        audibleHi = ADD.freqOf(d, k, f1) < 20000;
      if ((tool === "harm" || tool === "erase") && k < d.partials) {
        g.fillStyle = ADD.hoverK === k ? "rgba(93,162,232,0.22)" : "rgba(255,255,255,0.035)";
        g.fillRect(x + 1, G.top, bwid, G.bot - G.top);
      }
      if (k >= d.partials) continue;
      const edited = ADD.isEdited(d, k),
        v = Math.min(am[k], 1);
      g.fillStyle = !audible ? "#3a3a3a" : edited ? css("--human") : ADD.hoverK === k ? "#6fb3ad" : !audibleHi ? stripe(g, "#4a8f8a") : "#4a8f8a";
      g.fillRect(x + 1, G.bot - v * (G.bot - G.top), bwid, v * (G.bot - G.top));
      if (edited) {
        g.strokeStyle = "rgba(230,230,230,0.6)";
        g.setLineDash([2, 2]);
        g.strokeRect(x + 1.5, G.bot - base[k] * (G.bot - G.top), bwid - 1, base[k] * (G.bot - G.top));
        g.setLineDash([]);
      }
    }
    // 音域の高い端で弾いたときの強さ(輪郭の棒)
    if (ADD.hiDiffers(d)) {
      const amHi = ADD.amps({ ...d, previewNote: Rg.hi });
      g.strokeStyle = "rgba(240,200,120,0.95)";
      g.lineWidth = 1.2;
      for (let k = 0; k < d.partials; k++) {
        if (ADD.freqOf(d, k, f1) >= 20000) continue;
        const x = G.x0 + k * G.bw,
          bwid = Math.max(1, G.bw - 2),
          v = Math.min(amHi[k], 1);
        g.strokeRect(x + 1.5, G.bot - v * (G.bot - G.top), bwid - 1, v * (G.bot - G.top));
      }
      g.lineWidth = 1;
    }
    // 手で描いた山: 山ごとに範囲を青い帯で
    const handles = ADD.hillHandles(d, G, am);
    handles.forEach((hh: any) => {
      const pad = d.drawTo === "idx" ? 0.5 : 0.5 + (hh.hi + 1) * (Math.pow(2, 2 / 12) - 1);
      const a = G.x0 + clamp((hh.lo + 0.5 - pad) * G.bw, 0, P_MAX * G.bw),
        b2 = G.x0 + clamp((hh.hi + 0.5 + pad) * G.bw, 0, P_MAX * G.bw);
      g.fillStyle = ADD.hoverHill === hh.g || ADD.hillDrag?.g === hh.g ? "rgba(93,162,232,0.2)" : "rgba(93,162,232,0.09)";
      g.fillRect(a, G.top, Math.max(2, b2 - a), G.bot - G.top);
      g.fillStyle = "rgba(93,162,232,0.9)";
      g.fillRect(a, G.bot + 1, Math.max(2, b2 - a), 3);
    });
    // 鳴らさない所(倍音の数より右)と「ここまで鳴らす」の線
    const lx = G.x0 + d.partials * G.bw;
    if (d.partials < P_MAX) {
      g.save();
      g.beginPath();
      g.rect(lx, G.top, w - lx, G.bot - G.top);
      g.clip();
      g.strokeStyle = "rgba(255,255,255,0.06)";
      for (let i = lx - h; i < w; i += 8) {
        g.beginPath();
        g.moveTo(i, G.bot);
        g.lineTo(i + (G.bot - G.top), G.top);
        g.stroke();
      }
      g.restore();
    }
    const grabbable = tool !== "view",
      hot = ADD.hoverLine || ADD.dragLine;
    g.strokeStyle = hot ? css("--human") : css("--accent");
    g.lineWidth = hot ? 3 : 2;
    g.beginPath();
    g.moveTo(lx, G.top - 6);
    g.lineTo(lx, G.bot);
    g.stroke();
    g.lineWidth = 1;
    if (grabbable) {
      g.fillStyle = hot ? css("--human") : css("--accent");
      g.fillRect(lx - 5, G.top - 10, 10, 14);
      g.fillStyle = "#062523";
      g.fillRect(lx - 2, G.top - 7, 1, 8);
      g.fillRect(lx + 1, G.top - 7, 1, 8);
    }
    const fyTag = ADD.formantY(d, G) + 4,
      ptY = [G.top + 8, G.top + 26, G.top + 44].filter((y) => y <= G.bot - 14).find((y) => Math.abs(y - fyTag) >= 16);
    ADD._ptBox = tag(
      g,
      ptY != null ? tr(`${d.partials} 本まで鳴らす${grabbable ? "(線をつまんで増減)" : ""}`, `Up to ${d.partials} partials${grabbable ? " (drag the line)" : ""}`) : tr(`${d.partials} 本まで`, `Up to ${d.partials}`),
      lx + 8,
      ptY ?? G.top + 8,
      ptY != null ? w - 36 : Math.max(lx + 70, ADD.formantX(d, G) - 12),
      hot ? css("--human") : css("--accent"),
    );
    // 傾きの点線
    g.setLineDash([4, 4]);
    g.strokeStyle = css("--dim");
    g.lineWidth = 1;
    g.beginPath();
    ADD.base(d);
    const mN = ADD.lastM;
    for (let k = 1; k <= d.partials; k++) {
      const y = G.bot - clamp(Math.pow(k, d.tilt / 6.0206) / mN, 0, 1) * (G.bot - G.top);
      const x = G.x0 + (k - 0.5) * G.bw;
      if (k === 1) g.moveTo(x, y);
      else g.lineTo(x, y);
    }
    g.stroke();
    g.setLineDash([]);
    // ◆ にマウスを置いた・動かしているときは、◆ が動ける範囲(200〜5000 Hz)の外を薄く塞ぐ
    if (tool === "move" && (ADD.hoverFormant || ADD.dragFormant)) {
      const xOfHz = (hz: number) => G.x0 + clamp((ADD.kOfHz(d, hz, f0) + 0.5) * G.bw, 0, P_MAX * G.bw);
      const xl = xOfHz(200),
        xr = xOfHz(5000);
      g.fillStyle = "rgba(0,0,0,0.45)";
      g.fillRect(G.x0, G.top, xl - G.x0, G.bot - G.top);
      g.fillRect(xr, G.top, G.x0 + P_MAX * G.bw - xr, G.bot - G.top);
      g.strokeStyle = "rgba(93,162,232,0.7)";
      g.setLineDash([3, 3]);
      [xl, xr].forEach((x) => {
        if (x > G.x0 + 1 && x < G.x0 + P_MAX * G.bw - 1) {
          g.beginPath();
          g.moveTo(x, G.top);
          g.lineTo(x, G.bot);
          g.stroke();
        }
      });
      g.setLineDash([]);
      if (xr < G.x0 + P_MAX * G.bw - 1) tag(g, tr(`← 山はここまで(5000 Hz = ${noteName(d.previewNote ?? 60)} で ${Math.round(ADD.kOfHz(d, 5000, f0) + 1)} 番目)`, `← Hill limit (5000 Hz = partial ${Math.round(ADD.kOfHz(d, 5000, f0) + 1)} at ${noteName(d.previewNote ?? 60)})`), xr + 4, G.top + 30, w, css("--human"));
      if (xl - G.x0 > 60) tag(g, tr("200 Hz より下には置けない →", "Can't go below 200 Hz →"), G.x0 + 4, G.top + 48, w, css("--human"));
    }
    // ◆ の高さは「山の強さ」(0〜24 dB)。右端に専用の目盛り
    if (tool === "move") {
      g.font = "9.5px sans-serif";
      g.textAlign = "right";
      let lastFy = 1e9;
      [0, 6, 12, 18, 24].forEach((db) => {
        const y = ADD.formantY({ fDb: db }, G);
        g.strokeStyle = "rgba(93,162,232,0.5)";
        g.beginPath();
        g.moveTo(w - 6, y);
        g.lineTo(w - 1, y);
        g.stroke();
        if (lastFy - y < 11) return;
        lastFy = y;
        g.fillStyle = "rgba(93,162,232,0.85)";
        g.fillText(`+${db}`, w - 8, y + 3);
      });
      g.fillText(tr("◆ の強さ(dB)", "◆ strength (dB)"), w - 4, G.top - 6);
      g.textAlign = "left";
    }
    const fx = ADD.formantX(d, G),
      fy = ADD.formantY(d, G);
    g.fillStyle = tool === "move" ? css("--human") : css("--accent");
    g.beginPath();
    g.moveTo(fx, fy - 8);
    g.lineTo(fx + 8, fy);
    g.lineTo(fx, fy + 8);
    g.lineTo(fx - 8, fy);
    g.closePath();
    g.fill();
    const fk = ADD.formantK(d),
      fkHi = ADD.formantK({ ...d, previewNote: rangeOf(d).hi });
    if (fkHi < fk - 0.5) {
      const fxh = ADD.formantX({ ...d, previewNote: rangeOf(d).hi }, G);
      g.strokeStyle = "rgba(37,189,177,0.45)";
      g.lineWidth = 2;
      g.beginPath();
      g.moveTo(fxh, fy);
      g.lineTo(fx - 8, fy);
      g.stroke();
      g.lineWidth = 1;
      g.beginPath();
      g.moveTo(fxh, fy - 6);
      g.lineTo(fxh + 6, fy);
      g.lineTo(fxh, fy + 6);
      g.lineTo(fxh - 6, fy);
      g.closePath();
      g.stroke();
    }
    const kTxt = (k: number) => (k - 0.5 > P_MAX ? tr(`${P_MAX} 番目より上`, `above #${P_MAX}`) : k < 0.5 ? tr("1 番目より下", "below #1") : `${Math.max(1, Math.round(k))}`);
    const where = Math.round(fkHi) === Math.round(fk) ? tr(`${kTxt(fk)} 番目あたり`, `around #${kTxt(fk)}`) : tr(`${kTxt(fkHi)}〜${kTxt(fk)} 番目あたり`, `around #${kTxt(fkHi)}–${kTxt(fk)}`);
    {
      g.font = "11px sans-serif";
      const P = ADD._ptBox;
      const place = (ft: string) => {
        const tw = g.measureText(ft).width,
          lx2 = clamp(fx + 11, 2, w - tw - 8);
        const hits = (y: number) => P && lx2 - 3 < P[2] && P[0] < lx2 + tw + 3 && y - 11 < P[3] && P[1] < y + 4;
        return [fy + 4, fy - 14, fy + 22].find((y) => y - 11 >= 0 && y + 4 <= G.bot && !hits(y));
      };
      const full = tr(`フォルマント(◆)${Math.round(d.fHz)} Hz・+${d.fDb.toFixed(1)} dB・${where}`, `Formant (◆) ${Math.round(d.fHz)} Hz · +${d.fDb.toFixed(1)} dB · ${where}`),
        short = `◆ ${Math.round(d.fHz)} Hz・+${d.fDb.toFixed(1)} dB`;
      const yF = place(full),
        yS = yF == null ? place(short) : null;
      tag(g, yF != null ? full : short, fx + 11, yF ?? yS ?? fy + 4, w, tool === "move" ? css("--human") : css("--accent"));
    }
    // ◇: 手で描いた山のつまみ(山を動かすとき)
    if (tool === "move")
      handles.forEach((hh: any) => {
        const on = ADD.hoverHill === hh.g || ADD.hillDrag?.g === hh.g;
        g.beginPath();
        g.moveTo(hh.x, hh.y - 8);
        g.lineTo(hh.x + 8, hh.y);
        g.lineTo(hh.x, hh.y + 8);
        g.lineTo(hh.x - 8, hh.y);
        g.closePath();
        g.fillStyle = on ? css("--human") : "#141414";
        g.fill();
        g.strokeStyle = css("--human");
        g.lineWidth = 2;
        g.stroke();
        g.lineWidth = 1;
      });
    g.fillStyle = css("--faint");
    g.font = "10px sans-serif";
    axisRow(
      g,
      [1, 8, 16, 32, 48, 64].map((k) => [String(k), G.x0 + (k - 1) * G.bw] as [string, number]),
      h - 5,
      w,
    );
    refreshLegend("addLegend", ADD.legendItems(d));
    const info = byId("addInfo");
    if (info) {
      const k = ADD.hoverK,
        hh = handles.find((x: any) => x.g === (ADD.hillDrag?.g ?? ADD.hoverHill));
      const amHi = ADD.hiDiffers(d) ? ADD.amps({ ...d, previewNote: Rg.hi }) : null;
      info.textContent = hh
        ? tr(
            `手で描いた山(◇)· ${d.drawTo === "idx" ? `${Math.round(hh.lo) + 1}〜${Math.round(hh.hi) + 1} 番目(音と一緒に上下する)` : `${fmtHz(hh.pts[0].at)}〜${fmtHz(hh.pts[hh.pts.length - 1].at)} Hz(同じ周波数に残る)`} · いちばん上 ${hh.peak.db > 0 ? "+" : ""}${hh.peak.db.toFixed(1)} dB · 左右 = 位置、上下 = 強さ`,
            `Hand-drawn hill (◇) · ${d.drawTo === "idx" ? `partials ${Math.round(hh.lo) + 1}–${Math.round(hh.hi) + 1} (moves with the note)` : `${fmtHz(hh.pts[0].at)}–${fmtHz(hh.pts[hh.pts.length - 1].at)} Hz (stays at the same Hz)`} · peak ${hh.peak.db > 0 ? "+" : ""}${hh.peak.db.toFixed(1)} dB · across = position, up/down = strength`,
          )
        : k != null && k < d.partials
          ? tr(
              `${k + 1} 番目 · ${hzRange(ADD.freqOf(d, k, f0), ADD.freqOf(d, k, f1))} · ${(20 * Math.log10(Math.max(1e-6, am[k]))).toFixed(1)} dB${amHi && ADD.freqOf(d, k, f1) < 20000 ? `(${noteName(Rg.hi)} では ${(20 * Math.log10(Math.max(1e-6, amHi[k]))).toFixed(1)} dB)` : ""}${ADD.isEdited(d, k) ? ` · 手で ${ADD.offset(d, k) > 0 ? "+" : ""}${ADD.offset(d, k).toFixed(1)} dB` : ""}${ADD.freqOf(d, k, f0) >= 20000 ? " · 聞こえない" : ADD.freqOf(d, k, f1) >= 20000 ? ` · ${noteName(Rg.hi)} 側では聞こえない` : ""}`,
              `#${k + 1} · ${hzRange(ADD.freqOf(d, k, f0), ADD.freqOf(d, k, f1))} · ${(20 * Math.log10(Math.max(1e-6, am[k]))).toFixed(1)} dB${amHi && ADD.freqOf(d, k, f1) < 20000 ? ` (${(20 * Math.log10(Math.max(1e-6, amHi[k]))).toFixed(1)} dB at ${noteName(Rg.hi)})` : ""}${ADD.isEdited(d, k) ? ` · hand ${ADD.offset(d, k) > 0 ? "+" : ""}${ADD.offset(d, k).toFixed(1)} dB` : ""}${ADD.freqOf(d, k, f0) >= 20000 ? " · inaudible" : ADD.freqOf(d, k, f1) >= 20000 ? ` · inaudible at ${noteName(Rg.hi)}` : ""}`,
            )
          : (
              {
                view: tr("棒の上にマウスを置くと、番号・周波数・強さ", "Hover a bar for its number, frequency and level"),
                move: handles.length
                  ? tr("◆(フォルマント)か ◇(手で描いた山)をつまむ: 左右 = 位置、上下 = 強さ", "Drag ◆ (formant) or ◇ (hand-drawn hill): across = position, up/down = strength")
                  : tr("◆(フォルマント)をつまむ: 左右 = 位置(Hz。音域で当たる番目が変わる)、上下 = 強さ", "Drag ◆ (formant): across = position (Hz; the partial it hits depends on the range), up/down = strength"),
                harm: tr("なぞって描く。続けて描いた棒が 1 つの山になる(「山を動かす」の ◇ でつかめる)", "Drag to draw. Bars drawn in one stroke form one hill (grab it by ◇ with “Move hills”)"),
                erase: tr("なぞった所の、手で描いた分を消す(元の形に戻る)。縦線で本数", "Drag to remove what was hand-drawn (back to the base shape). The vertical line sets the count"),
              } as Record<string, string>
            )[tool];
    }
    // 倍音ごとの消え方: 横 = 時間(0〜3 秒)、縦 = 倍音、明るさ = 強さ
    const t = byId<HTMLCanvasElement>("addTime");
    if (t) {
      const { g: g2, w: w2, h: h2 } = ctx2d(t);
      const P = d.partials,
        T = 3,
        cols = 80;
      const lev = (k: number, time: number) => {
        const env = envAt(d as any, time);
        const dec = Math.exp((-time * d.damping * (k + 1)) / 1.5);
        return am[k] * env * (k ? dec : 1);
      };
      for (let k = 0; k < P; k++)
        for (let x = 0; x < cols; x++) {
          const v = lev(k, (x / cols) * T);
          const al = clamp((20 * Math.log10(Math.max(v, 1e-5)) + 48) / 48, 0, 1);
          g2.fillStyle = ADD.isEdited(d, k) ? `rgba(93,162,232,${al})` : `rgba(37,189,177,${al})`;
          g2.fillRect((x / cols) * w2, h2 - 12 - ((k + 1) / P) * (h2 - 12), w2 / cols + 1, (h2 - 12) / P + 1);
        }
      if (ADD.hoverK != null && ADD.hoverK < P) {
        const k = ADD.hoverK;
        g2.strokeStyle = "#fff";
        g2.lineWidth = 1.5;
        g2.strokeRect(1, h2 - 12 - ((k + 1) / P) * (h2 - 12), w2 - 2, Math.max(2, (h2 - 12) / P));
      }
      g2.fillStyle = css("--faint");
      g2.font = "10px sans-serif";
      g2.fillText(tr("0 秒", "0 s"), 2, h2 - 2);
      g2.fillText("1.5", w2 / 2 - 8, h2 - 2);
      g2.fillText(tr("3 秒", "3 s"), w2 - 24, h2 - 2);
      const ti = byId("addTimeInfo");
      if (ti) {
        if (ADD.hoverT) {
          const time = ADD.hoverT[0] * T,
            k = clamp(Math.floor(((h2 - 12 - (1 - ADD.hoverT[1]) * h2) / (h2 - 12)) * P), 0, P - 1);
          const v = lev(k, time);
          ti.textContent = tr(`${k + 1} 番目 · ${time.toFixed(2)} 秒 · ${v > 1e-4 ? (20 * Math.log10(v)).toFixed(0) + " dB" : "消えた"}`, `#${k + 1} · ${time.toFixed(2)} s · ${v > 1e-4 ? (20 * Math.log10(v)).toFixed(0) + " dB" : "gone"}`);
        } else ti.textContent = tr("上ほど高い倍音。明るいほど強い", "Higher partials toward the top; brighter = louder");
      }
    }
    // 周波数の並び: 薄い線 = 整数倍、色の線 = 実際の位置
    const f = byId<HTMLCanvasElement>("addFreq");
    if (f) {
      const { g: g3, w: w3, h: h3 } = ctx2d(f);
      const K = Math.min(d.partials, 24),
        xs = (r: number) => 8 + (r / (K + 1)) * (w3 - 16);
      for (let k = 1; k <= K; k++) {
        g3.fillStyle = "#333";
        g3.fillRect(xs(k), 6, 1, h3 - 22);
        const r = ADD.freqOf(d, k - 1, 1);
        const x = xs(r);
        if (x < w3) {
          g3.fillStyle = ADD.hoverK === k - 1 ? "#fff" : ADD.isEdited(d, k - 1) ? css("--human") : css("--accent");
          const a = Math.min(am[k - 1], 1);
          g3.fillRect(x - 1, 6 + (1 - a) * (h3 - 22), 2.5, a * (h3 - 22));
        }
      }
      g3.fillStyle = css("--faint");
      g3.font = "10px sans-serif";
      [1, 4, 8, 12, 16, 20, 24].filter((k) => k <= K).forEach((k) => g3.fillText(`×${k}`, xs(k) - 6, h3 - 3));
      const fi = byId("addFreqInfo");
      if (fi) {
        if (ADD.hoverF != null) {
          const k = clamp(Math.round(((ADD.hoverF * w3 - 8) / (w3 - 16)) * (K + 1)), 1, K);
          const r = ADD.freqOf(d, k - 1, 1);
          fi.textContent = tr(
            `${k} 番目 · 整数倍なら ×${k} → 実際 ×${r.toFixed(2)}(${r > k + 0.005 ? "+" + Math.round(1200 * Math.log2(r / k)) + " セント" : "ずれなし"})`,
            `#${k} · ×${k} if harmonic → actually ×${r.toFixed(2)} (${r > k + 0.005 ? "+" + Math.round(1200 * Math.log2(r / k)) + " cents" : "no drift"})`,
          );
        } else fi.textContent = tr("薄い線 = 整数倍、色の線 = 実際の位置", "Faint lines = whole multiples, colored = actual positions");
      }
    }
    ui.panes.forEach((p) => p.draw());
  },
  refresh() {
    rerender();
  },
  help: (): [string, string][] => [
    [tr("始めの形", "Starting shape"), tr("ベル・オルガン・声など。選ぶとつまみがその形になる(手で描いた山は消える)", "Bell, organ, voice… Picking one sets the knobs (hand-drawn hills are cleared)")],
    [tr("倍音のバランスを調整(下)", "Partial balance (below)"), tr("1 本 = 1 つの倍音。灰緑 = 元の形(右のつまみで決まる)、青 = 手で描いた山(点線が描く前の高さ)、灰色 = その音域では聞こえない。マウスの下の倍音は上の絵でも白く出る", "One bar per partial. Gray-green = base shape (set by the knobs), blue = hand-drawn (dotted = before drawing), gray = inaudible in the range. The hovered partial is white in the pictures above")],
    [tr("山を動かす", "Move hills"), tr("◆ = フォルマント、◇ = 手で描いた山。つまんで左右 = 位置、上下 = 強さ(手で描いた山は形のまま動く)", "◆ = formant, ◇ = hand-drawn hill. Drag across = position, up/down = strength (hills keep their shape)")],
    [tr("倍音の数", "Partial count"), tr("水色の縦線(ここまで鳴らす)。調整の道具のときに、線の上のつまみを左右にドラッグして増減。右の網の所は鳴らさない", "The teal vertical line (play up to here). With an editing tool, drag the tab on top to change it. The hatched area doesn't play")],
    [tr("棒を描く / 描いた山を消す", "Draw bars / Erase hills"), tr("なぞって描く / 消しゴムでなぞった所の、手で描いた分を消す。手で描いた分は元の形への上乗せ(dB)なので、後から右のつまみを動かしても効く", "Drag to draw / erase what was hand-drawn. Drawn amounts are added on top of the base shape (dB), so the knobs still work afterwards")],
    [
      tr("高さを変えたら、描いた山は", "When the pitch changes, hills"),
      tr("手で描いた山ぜんぶの性質(楽器ごとに 1 つ)。同じ周波数に残る = どの高さで弾いても同じ周波数が強い(声の母音・胴の響き。◆ と同じ)。音と一緒に上下する = どの高さでも同じ番目の倍音が強い。描いた後に切り替えても、今ある山がその場で変わる。ボタンにマウスを置くと違いを絵で見せる", "Applies to all hand-drawn hills (one setting per instrument). Stay at the same Hz = the same frequency is strong at any pitch (vowels, body resonance; like ◆). Move with the note = the same partial is strong at any pitch. Switching converts existing hills. Hover a button to see the difference"),
    ],
    [tr("倍音ごとの消え方", "How each partial fades"), tr("横 = 時間、縦 = 倍音。高い倍音ほど早く消える(高域の減衰)", "Across = time, up = partial. Upper partials fade sooner (high damping)")],
    [tr("倍音の位置のずれ", "Partial positions"), tr("非調和で高い倍音が上へずれる(ピアノ・鐘)。マウスの下のずれをセントで", "Inharmonicity pushes upper partials sharp (piano, bells). Shows the drift under the mouse in cents")],
    [tr("音量の変わり方", "Level over time"), tr("押してから離すまでの音量。「点をつまむ」で ○ を動かす", "Level from press to release. Use “Drag points” to move the ○")],
  ],
};

register(ADD);
export default ADD;
