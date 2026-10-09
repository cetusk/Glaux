// 音色エディタ: FM4(4 つのオペレーターの FM シンセ)。
// 左 = 始めの音・変調の構成(8 つ)、上 = 変調の構成の図(箱をクリックして選ぶ)、下 = 選んだ箱(出力 / 変調)の調整、右 = つまみ。
// 出口の音の倍音は、エンジンで 1 音を鳴らして測る(render_note_harmonics)。試作の近似の計算は使わない
import * as api from "../api";
import { tr } from "../i18n.svelte";
import { draw as drawAll, register, setSel, type Data, type Loaded } from "./core.svelte";
import { axisRow, byId, clamp, css, ctx2d, el, envAt, hzRange, ico, midiHz, stripe, tag } from "./dom";
import {
  canEdit,
  cur,
  editHead,
  envMax,
  envPane,
  explainOn,
  knob,
  knobLabel,
  legendRow,
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

/** 8 つの変調の構成(エンジンの fm4 と同じ。alg = algorithm − 1)。mods = [揺らす箱, 揺らされる箱]、car = 出口へ出す箱 */
const ALGS = (): { name: string; desc: string; mods: [number, number][]; car: number[] }[] => [
  { name: tr("直列", "Serial"), desc: tr("4→3→2→1 ブラス・リード", "4→3→2→1 brass, lead"), mods: [[3, 2], [2, 1], [1, 0]], car: [0] },
  { name: tr("混ぜて直列", "Mixed serial"), desc: tr("(4+3)→2→1 弦・ブラス", "(4+3)→2→1 strings, brass"), mods: [[3, 1], [2, 1], [1, 0]], car: [0] },
  { name: tr("直列と単独", "Serial + single"), desc: tr("(3→2 + 4)→1 エレピ", "(3→2 + 4)→1 e-piano"), mods: [[2, 1], [1, 0], [3, 0]], car: [0] },
  { name: tr("入れ替え", "Swapped"), desc: tr("(4→3 + 2)→1 ベース・木管", "(4→3 + 2)→1 bass, woodwind"), mods: [[3, 2], [2, 0], [1, 0]], car: [0] },
  { name: tr("2 組", "Two pairs"), desc: tr("2→1 と 4→3 エレピ・ベル", "2→1 and 4→3 e-piano, bell"), mods: [[1, 0], [3, 2]], car: [0, 2] },
  { name: tr("1 つで 3 つ", "One to three"), desc: tr("4→1・2・3 オルガン", "4→1, 2, 3 organ"), mods: [[3, 0], [3, 1], [3, 2]], car: [0, 1, 2] },
  { name: tr("1 組 + 2 つ", "Pair + two"), desc: tr("4→3 と 1・2 マリンバ", "4→3 and 1, 2 marimba"), mods: [[3, 2]], car: [0, 1, 2] },
  { name: tr("足す", "Additive"), desc: tr("1・2・3・4 オルガン・笛", "1, 2, 3, 4 organ, flute"), mods: [], car: [0, 1, 2, 3] },
];
type Op = { ratio: number; level: number; a: number; d: number; s: number };
const op = (ratio: number, level: number, a: number, d: number, s: number): Op => ({ ratio, level, a, d, s });
const FM4_PRESETS = (): [string, string, number, Op[], number, number][] => [
  ["epiano", tr("エレピ", "E-piano"), 4, [op(1, 0.9, 0.002, 1.2, 0.3), op(14, 0.35, 0.001, 0.15, 0), op(1, 0.8, 0.002, 1.5, 0.4), op(1, 0.5, 0.002, 0.8, 0.2)], 0.1, 0.5],
  ["bell", tr("ベル", "Bell"), 4, [op(1, 0.8, 0.001, 2.5, 0), op(3.5, 0.6, 0.001, 2, 0), op(1, 0.6, 0.001, 3, 0), op(2.7, 0.4, 0.001, 1.5, 0)], 0, 1.8],
  ["bass", tr("ベース", "Bass"), 3, [op(1, 0.9, 0.001, 0.6, 0.6), op(1, 0.55, 0.001, 0.25, 0.2), op(0.5, 0.4, 0.001, 0.3, 0.1), op(2, 0.2, 0.001, 0.2, 0)], 0.35, 0.15],
  ["brass", tr("ブラス", "Brass"), 0, [op(1, 0.9, 0.06, 0.5, 0.8), op(1, 0.6, 0.08, 0.6, 0.7), op(1, 0.3, 0.1, 0.4, 0.5), op(1, 0.2, 0.1, 0.4, 0.4)], 0.4, 0.3],
  ["organ", tr("オルガン", "Organ"), 7, [op(1, 0.8, 0.005, 0.1, 1), op(2, 0.5, 0.005, 0.1, 1), op(3, 0.35, 0.005, 0.1, 1), op(4, 0.25, 0.005, 0.1, 1)], 0, 0.08],
];
const FM4_RATIOS = [0.5, 1, 2, 3, 3.5, 4, 5, 7, 14];

/** 出口の音の倍音(エンジンで測った値)。鍵は音色と鳴らす高さ */
let hKey = "";
let hVal: { now: number[]; t0: number[] | null } | null = null;
let hPending = "";
let hTimer = 0;

const FM4: any = {
  kind: "fm4",
  title: () => "FM4",
  load({ params }: Loaded): Data {
    const n = (k: string, def: number) => (typeof params[k] === "number" ? params[k] : def);
    return {
      alg: clamp(Math.round(n("algorithm", 5)) - 1, 0, 7),
      ops: [1, 2, 3, 4].map((i) => op(n(`op${i}_ratio`, 1), n(`op${i}_level`, 1), n(`op${i}_attack`, 0.002), n(`op${i}_decay`, 1), n(`op${i}_sustain`, 0))),
      feedback: n("feedback", 0),
      release: n("release", 0.4),
      velBright: n("vel_bright", 0.6),
      gain: n("gain_db", -8),
    };
  },
  uiInit: (): Data => ({ sel: 0, previewNote: 60, specT: 0, showShelf: false, preset: null }),
  params(d: Data) {
    const p: Record<string, number> = { algorithm: d.alg + 1, feedback: d.feedback, release: d.release, vel_bright: d.velBright, gain_db: d.gain };
    d.ops.forEach((o: Op, i: number) => {
      p[`op${i + 1}_ratio`] = o.ratio;
      p[`op${i + 1}_level`] = o.level;
      p[`op${i + 1}_attack`] = o.a;
      p[`op${i + 1}_decay`] = o.d;
      p[`op${i + 1}_sustain`] = o.s;
    });
    return p;
  },
  hoverOp: null as number | null,
  hoverR: null as number | null,
  hoverH: null as number | null,
  render(body: HTMLElement) {
    const d = cur();
    const al = ALGS();
    body.innerHTML = "";
    // ---- 左: 始めの音・変調の構成 ----
    const left = el("div", { class: "col left", style: "width:250px" });
    const srcSeg = el("div", { class: "seg" });
    (
      [
        ["presets", tr("よくある音", "Common sounds")],
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
    const presets = el("div", { class: "chiprow" });
    FM4_PRESETS().forEach(([k, n, alg, ops, fb, rel]) =>
      presets.append(
        el(
          "button",
          {
            class: "btn" + (d.preset === k ? " on" : ""),
            onclick: () => {
              if (!canEdit()) return;
              Object.assign(d, { alg, ops: ops.map((o) => ({ ...o })), feedback: fb, release: rel, preset: k });
              pushHist(tr(`音を「${n}」に`, `Sound → "${n}"`));
              rerender();
            },
          },
          n,
        ),
      ),
    );
    left.append(el("div", { class: "box" }, el("h3", {}, tr("始めの音", "Starting sound")), srcSeg, d.showShelf ? shelfList("fm4") : presets));
    const list = el("div", { class: "shapes" });
    al.forEach((a, i) => {
      const c = el("canvas", { style: "height:46px" });
      list.append(
        el(
          "div",
          {
            class: "shape" + (d.alg === i ? " on" : ""),
            style: "grid-template-columns:70px 1fr",
            onclick: () => {
              if (!canEdit()) return;
              d.alg = i;
              pushHist(tr(`変調の構成を ${i + 1} に`, `Algorithm → ${i + 1}`));
              rerender();
            },
          },
          c,
          el("div", {}, `${i + 1}. ${a.name}`, el("small", {}, a.desc)),
        ),
      );
      requestAnimationFrame(() => FM4.diagram(c, i, true));
    });
    left.append(el("div", { class: "box" }, el("h3", {}, tr("変調の構成(アルゴリズム)", "Modulation layout (algorithm)")), list));
    // ---- 真ん中 ----
    // 低い画面では真ん中の列ごとスクロール。上は 4 段の構成でも箱が潰れない高さ、下は変わり方の欄のつまみ(2 段)まで入る高さ
    const center = el("div", { class: "col center wtcol", style: "overflow-y: auto" });
    const grid = el("div", { class: "wtgrid", style: "grid-template-rows: minmax(min-content, 1fr) minmax(296px, 1.25fr)" });
    const dg = el("canvas", { id: "fmDiag", class: "fill", style: "cursor:pointer; min-height: 150px", title: tr("出力・変調の箱(オペレーター)をクリックして、下で調整する物を選ぶ", "Click an output / modulator box (operator) to edit it below") });
    const dinfo = el("div", { class: "paneinfo", id: "fmDiagInfo" });
    grid.append(
      el(
        "div",
        { class: "box", style: "grid-column: span 4" },
        el("h3", {}, tr(`変調の構成 ${d.alg + 1}: ${al[d.alg].name}`, `Algorithm ${d.alg + 1}: ${al[d.alg].name}`), el("span", { class: "spacer" }), FM4.howBtn()),
        dg,
        dinfo,
        legendRow([
          ["car", tr("出力(出口へ出す)", "Output (to the out)"), tr("キャリア: 自分の波を出口へ出す。出口から出た音が、このトラックの音になる。音量と音量の変わり方を持つ", "Carrier: sends its wave to the out, which becomes this track's sound. Has a level and level envelope")],
          ["mod", tr("変調(音色を作る)", "Modulator (shapes the tone)"), tr("モジュレーター: 聞こえない。矢印の先の波を揺らして倍音を作る", "Modulator: not heard. Wobbles the wave it points to, making harmonics")],
          [el("span", { class: "sw arrow" }), tr("変調の向き(太さ = 強さ)", "Modulation direction (width = depth)")],
          ["fb", tr("自分を揺らす(4 番)", "Self-modulation (#4)")],
          ["now", tr("下で調整中", "Editing below")],
        ]),
      ),
    );
    grid.append(specBox("grid-column: span 2"));
    // 下: 選んだ箱を調整
    const o: Op = d.ops[d.sel],
      isCar = al[d.alg].car.includes(d.sel);
    const targets = al[d.alg].mods.filter(([m]) => m === d.sel).map(([, t]) => FM4.nm(d.alg, t));
    const ratioC = el("canvas", { id: "fmRatio", class: "fill" });
    // マウスを置くと文が変わるので 2 行分を取っておく(下のボタンが動かないように)
    const rinfo = el("span", { class: "hint", id: "fmRatioInfo", style: "min-height: 2lh" });
    const chips = el("div", { class: "chiprow" });
    FM4_RATIOS.forEach((r) =>
      chips.append(
        el(
          "button",
          {
            class: "btn" + (o.ratio === r ? " on" : ""),
            onclick: () => {
              if (!canEdit()) return;
              o.ratio = r;
              pushHist(tr(`${FM4.nm(d.alg, d.sel)} の高さの倍率を ${r} に`, `${FM4.nm(d.alg, d.sel)} ratio → ${r}`));
              rerender();
            },
          },
          `×${r}`,
        ),
      ),
    );
    const spec = el("canvas", { id: "fmSpec", class: "fill" });
    const sinfo = el("span", { class: "hint", id: "fmSpecInfo", style: "min-height: 1lh" });
    const tSeg = toolSeg(
      [
        ["0", tr("鳴り始め", "At the start"), tr("鍵盤を押して、いちばん大きくなった瞬間の倍音", "Harmonics at the loudest moment after the key press"), "attack"],
        ["0.5", tr("0.5 秒後", "0.5 s later"), tr("0.5 秒たった時の倍音(変調の強さが時間で変わると、音色も変わる)", "Harmonics 0.5 s later (if modulation depth changes over time, so does the tone)"), "later"],
      ],
      String(d.specT ?? 0),
      (t) => {
        d.specT = +t;
        rerender();
      },
    );
    grid.append(
      el(
        "div",
        { class: "box editgroup", style: "grid-column: span 6" },
        editHead(
          tr(`${FM4.nm(d.alg, d.sel)} を調整`, `Edit ${FM4.nm(d.alg, d.sel)}`),
          isCar ? FM4.carNote(d.alg, d.sel) : tr(`${targets.join("・") || "—"} の音色を作る`, `Shapes the tone of ${targets.join(", ") || "—"}`),
          isCar ? "car" : "mod",
          pitchPick(d, () => rerender()),
        ),
        el(
          "div",
          { class: "editpanes three" },
          pane(tr("高さの倍率", "Ratio"), {
            info: rinfo,
            tools: toolSeg([VIEW_TOOL(), ["points", tr("左右にずらす", "Slide"), tr("▲ を左右にずらす(0.5 ずつ。Alt を押すと細かく)", "Slide ▲ left/right (by 0.5; hold Alt for fine steps)"), "spread"]], toolOf("ratio"), (t) => setTool("ratio", t)),
            canvas: ratioC,
            below: chips,
          }),
          envPane("fmEnv", {
            // 右のつまみ(音量 / 変調の強さ)と同じ名前。どの箱かは枠の見出し「◯◯ を調整」で分かる
            title: isCar ? tr("音量の変わり方", "Level over time") : tr("変調の強さの変わり方", "Mod depth over time"),
            get: () => {
              const D = shown(),
                p = D.ops[D.sel];
              return { a: p.a, d: p.d, s: p.s, r: D.release };
            },
            set: (k, v) => {
              const D = cur();
              if (k === "r") D.release = v;
              else D.ops[D.sel][k] = v;
            },
            color: isCar ? css("--accent") : css("--human"),
            note: tr("余韻は 4 つ共通", "release is shared by all four"),
            max: { a: 5, d: 10, r: 8 },
            below: knob(isCar ? "音量" : "変調の強さ", {
              cls: "wide",
              title: isCar ? tr("この出力の音量(元の名前: 出力レベル)", "This output's level (output level)") : tr("矢印の先の波を揺らす強さ。上げるほど倍音が増える(元の名前: 出力レベル・変調の深さ)", "How strongly it wobbles the wave it points to; higher = more harmonics (output level / mod index)"),
              bind: `${state.track}:op${d.sel}:level`,
              min: 0,
              max: 1,
              step: 0.01,
              value: o.level,
              onInput: (v) => {
                o.level = v;
                drawAll();
              },
              onCommit: (v) => pushHist(tr(`${FM4.nm(d.alg, d.sel)} の${isCar ? "音量" : "変調の強さ"}を ${v} に`, `${FM4.nm(d.alg, d.sel)} ${knobLabel(isCar ? "音量" : "変調の強さ")} → ${v}`)),
            }),
          }),
          pane(tr("出口の音の倍音", "Harmonics at the out"), { info: sinfo, tools: tSeg, canvas: spec, below: liveLegend("fmSpecLegend", FM4.specLegend(d)) }),
        ),
      ),
    );
    center.append(grid);
    // ---- 右: つまみ ----
    const right = el("div", { class: "col right" });
    const k = (obj: any, label: string, key: string, min: number, max: number, step: number, unit = "") =>
      knob(label, {
        min,
        max,
        step,
        value: obj[key],
        unit,
        bind: obj === d ? `${state.track}:${key}` : `${state.track}:op${d.sel}:${key}`,
        onInput: (v) => {
          obj[key] = v;
          drawAll();
        },
        onCommit: (v) => pushHist(tr(`${obj === d ? "" : `${FM4.nm(d.alg, d.sel)} の`}${label}を ${v}${unit} に`, `${obj === d ? "" : `${FM4.nm(d.alg, d.sel)} `}${knobLabel(label)} → ${v}${unit}`)),
      });
    right.append(
      el(
        "div",
        { class: "box" },
        el("h3", {}, FM4.nm(d.alg, d.sel), el("span", { class: "pill " + (isCar ? "car" : "mod") }, isCar ? tr("出口へ出す", "To the out") : tr("音色を作る", "Shapes the tone"))),
        k(o, "高さの倍率", "ratio", 0.5, 16, 0.01),
        k(o, isCar ? "音量" : "変調の強さ", "level", 0, 1, 0.01),
        k(o, "立ち上がり", "a", 0.001, envMax("fmEnv", 5), 0.001, " s"),
        k(o, "下がる時間", "d", 0.01, envMax("fmEnv", 10), 0.01, " s"),
        k(o, "保つ音量", "s", 0, 1, 0.01),
      ),
    );
    right.append(
      el(
        "div",
        { class: "box" },
        el("h3", {}, tr("全体", "Overall")),
        k(d, "自分を揺らす", "feedback", 0, 1, 0.01),
        k(d, "余韻", "release", 0.01, envMax("fmEnv", 8), 0.01, " s"),
        k(d, "強さで明るさ", "velBright", 0, 1, 0.01),
        k(d, "ゲイン", "gain", -24, 6, 0.5, " dB"),
      ),
    );
    right.append(saveBox(state.trackName, tr("変調の構成と 4 つの出力・変調", "the layout and all four operators")));
    body.append(left, center, right);
    // ---- 変調の構成の図: クリックで選ぶ・マウスの下の箱の説明 ----
    const hitOp = (ev: PointerEvent) => {
      const r = dg.getBoundingClientRect(),
        x = ev.clientX - r.left,
        y = ev.clientY - r.top;
      return FM4.boxes(r.width, r.height, d.alg).findIndex((b: any) => x >= b.x && x <= b.x + b.w && y >= b.y && y <= b.y + b.h);
    };
    dg.addEventListener("pointerdown", (ev) => {
      const i = hitOp(ev);
      if (i >= 0) {
        d.sel = i;
        setSel(FM4.nm(d.alg, i));
        rerender();
      }
    });
    dg.addEventListener("pointermove", (ev) => {
      const i = hitOp(ev);
      FM4.hoverOp = i >= 0 ? i : null;
      dg.style.cursor = i >= 0 ? "pointer" : "default";
      FM4.draw();
    });
    dg.addEventListener("pointerleave", () => {
      FM4.hoverOp = null;
      FM4.draw();
    });
    // ---- 高さの倍率の目盛り: 目盛りと同じ左右 8 px の余白 ----
    let rdrag = false;
    const ratioAt = (ev: PointerEvent) => {
      const r = ratioC.getBoundingClientRect();
      return FM4.ratioOf((ev.clientX - r.left - 8) / (r.width - 16));
    };
    const snapR = (ev: PointerEvent, v: number) => (ev.altKey ? Math.round(v * 100) / 100 : Math.max(0.5, Math.round(v * 2) / 2));
    ratioC.addEventListener("pointerdown", (ev) => {
      if (toolOf("ratio") !== "points" || !canEdit()) return;
      rdrag = true;
      ratioC.setPointerCapture(ev.pointerId);
      o.ratio = snapR(ev, ratioAt(ev));
      drawAll();
    });
    ratioC.addEventListener("pointermove", (ev) => {
      FM4.hoverR = ratioAt(ev);
      if (rdrag) o.ratio = snapR(ev, FM4.hoverR);
      ratioC.style.cursor = toolOf("ratio") === "points" ? "ew-resize" : "default";
      drawAll();
    });
    ratioC.addEventListener("pointerup", () => {
      if (!rdrag) return;
      rdrag = false;
      pushHist(tr(`${FM4.nm(d.alg, d.sel)} の高さの倍率を ${o.ratio} に`, `${FM4.nm(d.alg, d.sel)} ratio → ${o.ratio}`));
      rerender();
    });
    ratioC.addEventListener("pointerleave", () => {
      FM4.hoverR = null;
      FM4.draw();
    });
    // 棒 j(= (j + 1) / 2 倍)は x = j / 80 の所にある
    spec.addEventListener("pointermove", (ev) => {
      const r = spec.getBoundingClientRect();
      FM4.hoverH = clamp(Math.round(((ev.clientX - r.left) / r.width) * 80), 0, 79);
      FM4.draw();
    });
    spec.addEventListener("pointerleave", () => {
      FM4.hoverH = null;
      FM4.draw();
    });
    // 「音量の変わり方」と「変調の強さの変わり方」で見出しの折り返しが変わると下の絵がずれるので、長いほうの高さを取っておく
    requestAnimationFrame(() => {
      const head = byId("fmEnv")?.closest(".pane")?.querySelector<HTMLElement>(".panehead"),
        t = head?.firstChild as HTMLElement | null;
      if (!head || !t) return;
      const now = t.textContent;
      t.textContent = tr("変調の強さの変わり方", "Mod depth over time");
      const hh = head.offsetHeight;
      t.textContent = now;
      head.dataset.reserve = String(hh);
      head.style.minHeight = `${hh}px`;
    });
    // 構成・よくある音を変えても、選んでいる所の名前を今の役に合わせる
    setSel(FM4.nm(d.alg, d.sel));
  },
  /** 出口の音の倍音の凡例: 絵に出ている色だけ */
  specLegend(d: Data): LegendItem[] {
    const h = FM4.harm(d);
    if (!h) return [["base", tr("整数倍", "Whole multiples")]];
    const t = d.specT ?? 0,
      am = h.now,
      m = Math.max(...(t ? (h.t0 ?? am) : am)) || 1,
      R = rangeOf(d),
      f0 = midiHz(R.lo),
      f1 = midiHz(R.hi);
    const seen = am.map((a: number, j: number) => ({ hm: (j + 1) / 2, on: a / m > 0.004 })).filter((q: any) => q.on && q.hm <= 40);
    const half = seen.some((q: any) => !Number.isInteger(q.hm)),
      muteLo = seen.some((q: any) => f0 * q.hm >= 20000),
      muteHi = seen.some((q: any) => f0 * q.hm < 20000 && f1 * q.hm >= 20000);
    return [
      ["base", tr("整数倍", "Whole multiples")],
      ...(half ? [[el("span", { class: "sw", style: "background: var(--warn); width: 5px" }), tr("半整数倍(7.5 倍など)", "Half multiples (7.5× etc.)")] as LegendItem] : []),
      ...(muteHi ? [["mute2", tr("高い側で聞こえない", "Silent at the high end"), tr("音域の高い端の鍵盤では 20 kHz を超えて聞こえない(低い端では鳴る)", "Above 20 kHz at the top of the range (heard at the bottom)")] as LegendItem] : []),
      ...(muteLo ? [["mute", tr("聞こえない", "Not heard"), tr("この音域では 20 kHz を超えて聞こえない", "Above 20 kHz in this range")] as LegendItem] : []),
      ...(t ? [[el("span", { class: "sw dash" }), tr("鳴り始めの高さ", "Level at the start")] as LegendItem] : []),
    ];
  },
  /** 箱の名前: 役(出力 = キャリア・聞こえる / 変調 = モジュレーター・音色を作る)+ 番号。役は変調の構成で変わる */
  nm: (alg: number, i: number) => (ALGS()[alg].car.includes(i) ? tr(`出力 ${i + 1}`, `Out ${i + 1}`) : tr(`変調 ${i + 1}`, `Mod ${i + 1}`)),
  /** 出力の札: 出口へ出すこと + その音色を作っている変調(無ければサイン波のまま) */
  carNote: (alg: number, i: number) => {
    const by = ALGS()[alg].mods.filter(([, t]) => t === i).map(([m]) => FM4.nm(alg, m));
    return by.length ? tr(`出口へ出す · 音色は ${by.join("・")} が作る`, `To the out · tone shaped by ${by.join(", ")}`) : tr("出口へ出す · サイン波のまま", "To the out · a plain sine");
  },
  /** 「仕組み」: マウスを置くと、変調が出力の波をどう変えるかを小さな絵で見せる */
  howBtn() {
    const b = el("span", { class: "howbtn" }, ico("info"), tr("仕組み", "How it works"));
    queueMicrotask(() =>
      explainOn(b, "fmhow", (pop) => {
        const c = el("canvas", { style: "height:170px" });
        pop.append(
          el("b", {}, tr("出力・変調と矢印の意味", "Outputs, modulators and arrows")),
          c,
          el(
            "div",
            { class: "small" },
            tr(
              "4 つの箱はどれもサイン波を 1 本出す部品(オペレーター)です。役は 2 つ。「出力」(キャリア)は出口につながり、そのまま聞こえます。「変調」(モジュレーター)は聞こえず、矢印の先の波の周波数を揺らして形を変えます(FM ラジオと同じ周波数変調)。揺らすほど倍音が増え、明るく・金属っぽくなります。どの箱がどの役かは、変調の構成で決まります。",
              "Each of the four boxes (operators) makes one sine wave. There are two roles. An “output” (carrier) goes to the out and is heard directly. A “modulator” isn't heard; it wobbles the frequency of the wave it points to, changing its shape (frequency modulation, as in FM radio). More wobble means more harmonics — brighter and more metallic. The layout decides which box plays which role.",
            ),
          ),
        );
        requestAnimationFrame(() => {
          const { g, w, h } = ctx2d(c);
          const rows: [string, (p: number) => number][] = [
            [tr("変調だけ(サイン波)", "Modulator alone (sine)"), (p) => Math.sin(2 * Math.PI * 2 * p)],
            [tr("出力だけ(サイン波)", "Output alone (sine)"), (p) => Math.sin(2 * Math.PI * p)],
            [tr("矢印で変調すると(波の形が変わる)", "Modulated by the arrow (the shape changes)"), (p) => Math.sin(2 * Math.PI * p + 2.5 * Math.sin(2 * Math.PI * 2 * p))],
          ];
          rows.forEach(([label, f], i) => {
            const y0 = i * (h / 3) + 12,
              hh = h / 3 - 18;
            g.fillStyle = i === 2 ? css("--accent") : css("--dim");
            g.font = "10.5px sans-serif";
            g.fillText(label, 4, y0);
            g.strokeStyle = i === 0 ? css("--human") : css("--accent");
            g.lineWidth = 1.6;
            g.beginPath();
            for (let n = 0; n <= 200; n++) {
              const p = (n / 200) * 2,
                x = 4 + (n / 200) * (w - 8),
                y = y0 + 4 + hh / 2 - f(p) * (hh / 2 - 2);
              if (n) g.lineTo(x, y);
              else g.moveTo(x, y);
            }
            g.stroke();
          });
        });
      }),
    );
    return b;
  },
  // 高さの倍率の目盛り: 0.5〜16 を対数で
  ratioX(r: number) {
    return Math.log(r / 0.5) / Math.log(32);
  },
  ratioOf(fx: number) {
    return 0.5 * Math.pow(32, clamp(fx, 0, 1));
  },
  boxes(w: number, h: number, alg: number, mini = false) {
    // 段: 変調の深さで縦に並べる(出力がいちばん下)
    const al = ALGS()[alg],
      level = [0, 0, 0, 0];
    const depth = (i: number): number => {
      const outs = al.mods.filter(([m]) => m === i).map(([, c]) => c);
      return al.car.includes(i) ? 0 : 1 + Math.max(...outs.map(depth));
    };
    for (let i = 0; i < 4; i++) level[i] = depth(i);
    // 段の間: 低い絵では詰めて、箱の高さ(30 px 以上)を先に取る
    const foot = mini ? 8 : 30,
      gap = mini ? 3 : clamp((h - foot - 4 - (Math.max(...level) + 1) * 30) / Math.max(1, Math.max(...level)), 12, 26);
    // 箱の幅: いちばん多く横に並ぶ段でも、左右の余白と自分を揺らす輪(右に 11 px)まで枠に収まる幅
    const gx = mini ? 6 : 34,
      side = mini ? 6 : 20,
      maxN = Math.max(...[0, 1, 2, 3].map((L) => level.filter((v) => v === L).length));
    const maxL = Math.max(...level),
      bw = Math.min(mini ? 18 : 130, w / 5, (w - side * 2 - (maxN - 1) * gx) / maxN),
      bh = Math.min(mini ? 9 : 64, (h - foot - 4 - maxL * gap) / (maxL + 1));
    const rows: Record<number, number> = {};
    return [0, 1, 2, 3]
      .map((i) => {
        const L = level[i];
        rows[L] = (rows[L] ?? 0) + 1;
        return { i, L, k: rows[L] - 1 };
      })
      .map((o) => {
        const n = [0, 1, 2, 3].filter((j) => level[j] === o.L).length;
        const x = w / 2 + (o.k - (n - 1) / 2) * (bw + gx) - bw / 2;
        const y = h - foot - bh - o.L * (bh + gap);
        return { x, y, w: bw, h: bh, i: o.i };
      });
  },
  diagram(c: HTMLCanvasElement, alg: number, mini: boolean) {
    const d = shown();
    if (!d) return;
    const { g, w, h } = ctx2d(c);
    const bx = FM4.boxes(w, h, alg, mini);
    const al = ALGS()[alg];
    // 変調の矢印(太さ = 変調の強さ)
    al.mods.forEach(([m, t]) => {
      const a = bx[m],
        b = bx[t];
      g.strokeStyle = mini ? css("--dim") : css("--human");
      g.lineWidth = mini ? 1 : 1 + d.ops[m].level * 4;
      g.beginPath();
      g.moveTo(a.x + a.w / 2, a.y + a.h);
      g.lineTo(b.x + b.w / 2, b.y);
      g.stroke();
      if (!mini) {
        g.fillStyle = css("--human");
        g.beginPath();
        g.moveTo(b.x + b.w / 2, b.y);
        g.lineTo(b.x + b.w / 2 - 5, b.y - 8);
        g.lineTo(b.x + b.w / 2 + 5, b.y - 8);
        g.fill();
      }
    });
    // 出口
    const outY = mini ? h - 6 : h - 20;
    al.car.forEach((i) => {
      const b = bx[i];
      g.strokeStyle = css("--accent");
      g.lineWidth = mini ? 1 : 2;
      g.beginPath();
      g.moveTo(b.x + b.w / 2, b.y + b.h);
      g.lineTo(b.x + b.w / 2, outY);
      g.stroke();
    });
    g.fillStyle = css("--accent");
    g.fillRect(w * 0.2, outY, w * 0.6, 2);
    if (!mini) {
      g.font = "11px sans-serif";
      g.fillText(tr("出口(音)", "Out (sound)"), w * 0.2, outY + 14);
    }
    // 自分を揺らす(4 番)
    const b4 = bx[3];
    g.strokeStyle = css("--warn");
    g.lineWidth = mini ? 1 : 1 + d.feedback * 3;
    g.beginPath();
    g.arc(b4.x + b4.w, b4.y + b4.h / 2, mini ? 4 : 11, -Math.PI / 2, Math.PI / 2);
    g.stroke();
    bx.forEach((b: any) => {
      const sel = !mini && d.sel === b.i,
        car = al.car.includes(b.i),
        hov = !mini && FM4.hoverOp === b.i;
      g.fillStyle = sel ? "rgba(255,255,255,0.08)" : hov ? "#303030" : "#262626";
      g.strokeStyle = sel ? "#ffffff" : car ? css("--accent") : css("--human");
      g.lineWidth = sel ? 2.5 : 1;
      g.fillRect(b.x, b.y, b.w, b.h);
      g.strokeRect(b.x, b.y, b.w, b.h);
      g.fillStyle = css("--text");
      g.font = mini ? "9px sans-serif" : "bold 12px sans-serif";
      g.fillText(mini ? String(b.i + 1) : FM4.nm(alg, b.i), b.x + (mini ? 5 : 6), b.y + (mini ? 8 : 16));
      if (!mini) {
        const o = d.ops[b.i];
        g.font = "bold 12px sans-serif";
        const nameW = g.measureText(FM4.nm(alg, b.i)).width;
        g.font = "11px sans-serif";
        const rw = g.measureText(`×${o.ratio}`).width;
        g.fillStyle = css("--dim");
        // 狭い箱では省く(下の欄・マウスの下の行に出る)
        if (6 + nameW + 8 + rw + 6 <= b.w) g.fillText(`×${o.ratio}`, b.x + b.w - rw - 6, b.y + 16);
        // 小さな包絡の絵(箱が低いときは省く)
        const ex = b.x + 6,
          ew = b.w - 12,
          ey = b.y + 22,
          eh = b.h - 34;
        if (eh >= 8) {
          g.strokeStyle = car ? css("--accent") : css("--human");
          g.lineWidth = 1.2;
          g.beginPath();
          for (let i = 0; i <= 30; i++) {
            const t = (i / 30) * 1.5,
              v = envAt(o, t) * o.level;
            const x = ex + (i / 30) * ew,
              y = ey + eh - v * eh;
            if (i) g.lineTo(x, y);
            else g.moveTo(x, y);
          }
          g.stroke();
        }
        // 低い箱では名前と重なるので省く
        if (b.h >= 28) {
          g.fillStyle = car ? css("--accent") : css("--human");
          g.fillRect(b.x + 6, b.y + b.h - 8, (b.w - 12) * o.level, 4);
        }
      }
    });
    if (!mini) {
      const b = bx[d.sel];
      g.font = "11px sans-serif";
      const label = tr("下で調整中", "Editing below");
      const tw = g.measureText(label).width + 6;
      // どの箱(自分も)にも重ねない
      const free = (x: number, y: number) => x >= 0 && x + tw <= w && y - 11 >= 0 && y + 4 <= h && bx.every((o: any) => x + tw < o.x || x > o.x + o.w || y + 4 < o.y || y - 11 > o.y + o.h);
      const spot = (
        [
          [b.x, b.y - 6],
          [b.x, b.y + b.h + 16],
          [b.x + b.w + 8, b.y + 14],
        ] as [number, number][]
      ).find(([x, y]) => free(x, y));
      if (spot) tag(g, label, spot[0], spot[1], w);
    }
  },
  /** 出口の音の倍音: エンジンで測った値(無ければ測りに行き、届いたら描き直す)。am[j] は (j + 1) / 2 倍 */
  harm(d: Data): { now: number[]; t0: number[] | null } | null {
    const t = d.specT ?? 0;
    const pitch = rangeOf(d).lo;
    const key = JSON.stringify([FM4.params(d), pitch, t]);
    if (key === hKey) return hVal;
    if (key !== hPending) {
      hPending = key;
      clearTimeout(hTimer);
      hTimer = window.setTimeout(async () => {
        const tr0 = state.project?.tracks.find((x) => x.id === state.track);
        const device = { ...(tr0?.device ?? { type: "builtin", name: "fm4" }), params: { ...(tr0?.device?.params ?? {}), ...FM4.params(d) } };
        try {
          const times = t ? [t, 0] : [0];
          const r = await api.renderNoteHarmonics(state.track, pitch, times, { device, step: 0.5, maxMult: 40 });
          if (hPending !== key) return;
          hKey = key;
          hVal = { now: r[0], t0: t ? r[1] : null };
          FM4.draw();
        } catch {
          hPending = "";
        }
      }, 60);
    }
    return hVal;
  },
  draw() {
    const d = shown();
    if (!d) return;
    const al = ALGS();
    const c = byId<HTMLCanvasElement>("fmDiag");
    if (c) FM4.diagram(c, d.alg, false);
    const di = byId("fmDiagInfo");
    if (di) {
      const i = FM4.hoverOp;
      if (i != null) {
        const o = d.ops[i],
          car = al[d.alg].car.includes(i);
        const to = al[d.alg].mods.filter(([m]) => m === i).map(([, t]) => FM4.nm(d.alg, t));
        di.textContent = `${FM4.nm(d.alg, i)} · ${car ? FM4.carNote(d.alg, i) : tr(`${to.join("・")} の音色を作る`, `shapes the tone of ${to.join(", ")}`)} · ×${o.ratio} · ${knobLabel(car ? "音量" : "変調の強さ")} ${o.level.toFixed(2)}${i === 3 ? ` · ${knobLabel("自分を揺らす")} ${d.feedback.toFixed(2)}` : ""}`;
      } else di.textContent = tr("出力・変調の箱をクリックすると、下で調整できます", "Click an output / modulator box to edit it below");
    }
    // 高さの倍率の目盛り
    const rc = byId<HTMLCanvasElement>("fmRatio");
    if (rc) {
      const { g, w, h } = ctx2d(rc);
      const o = d.ops[d.sel],
        f0 = midiHz(d.previewNote ?? 60),
        f1 = midiHz(rangeOf(d).hi);
      // 低い絵でも下の数字が入るように
      const xs = (r: number) => 8 + FM4.ratioX(r) * (w - 16),
        mid = Math.min(h / 2, h - 24);
      g.strokeStyle = "#3a3a3a";
      g.lineWidth = 1;
      g.beginPath();
      g.moveTo(8, mid);
      g.lineTo(w - 8, mid);
      g.stroke();
      g.font = "10px sans-serif";
      for (let r = 1; r <= 16; r++) {
        const x = xs(r);
        g.fillStyle = "#555";
        g.fillRect(x, mid - (r % 2 ? 6 : 4), 1, r % 2 ? 12 : 8);
      }
      g.fillStyle = css("--faint");
      axisRow(
        g,
        ([[0.5, xs(0.5) - 6], ...[1, 2, 3, 4, 6, 8, 12, 16].map((r) => [r, xs(r) - 3])] as [number, number][]).map(([r, x]) => [String(r), x] as [string, number]),
        Math.min(mid + 20, h - 3),
        w,
        4,
      );
      if (FM4.hoverR != null) {
        const x = 8 + FM4.ratioX(FM4.hoverR) * (w - 16);
        g.strokeStyle = toolOf("ratio") === "points" ? "rgba(93,162,232,0.5)" : "rgba(255,255,255,0.18)";
        g.beginPath();
        g.moveTo(x, 6);
        g.lineTo(x, h - 6);
        g.stroke();
      }
      const x = xs(o.ratio);
      g.fillStyle = toolOf("ratio") === "points" ? css("--human") : css("--accent");
      g.beginPath();
      g.moveTo(x, mid - 2);
      g.lineTo(x - 7, mid - 14);
      g.lineTo(x + 7, mid - 14);
      g.closePath();
      g.fill();
      // 低い絵でも上で切れない
      tag(g, `×${o.ratio} = ${hzRange(f0 * o.ratio, f1 * o.ratio)}`, x - 30, Math.max(13, mid - 20), w, css("--text"));
      const ri = byId("fmRatioInfo");
      const kindOf = (r: number) =>
        Number.isInteger(r)
          ? tr("整数倍(楽器らしい音)", "Whole multiple (instrument-like)")
          : r * 2 === Math.round(r * 2)
            ? tr("半整数倍(1 オクターブ下にも倍音)", "Half multiple (harmonics an octave below too)")
            : tr("非整数倍(鐘・金属の音)", "Non-integer (bell, metallic)");
      // マウスの下の倍率
      if (ri) {
        const hr = FM4.hoverR,
          hv = hr == null ? null : toolOf("ratio") === "points" ? Math.max(0.5, Math.round(hr * 2) / 2) : Math.round(hr * 100) / 100;
        ri.textContent = hv != null ? `×${hv} · ${hzRange(f0 * hv, f1 * hv)} · ${kindOf(hv)}` : kindOf(o.ratio);
      }
    }
    // 出口の音の倍音
    const s = byId<HTMLCanvasElement>("fmSpec");
    if (s) {
      const { g, w, h } = ctx2d(s);
      const H = FM4.harm(d);
      const si = byId("fmSpecInfo");
      if (!H) {
        g.fillStyle = css("--faint");
        g.font = "11px sans-serif";
        g.fillText(tr("鳴らして測っています…", "Measuring…"), 8, h / 2);
        if (si) si.textContent = "";
      } else {
        const t = d.specT ?? 0,
          am = H.now,
          am0 = t ? H.t0 : null;
        const K = 40,
          bw = w / K,
          f0 = midiHz(d.previewNote ?? 60),
          f1 = midiHz(rangeOf(d).hi),
          m = Math.max(...(am0 ?? am)) || 1;
        const yOf = (v: number) => {
          const db = 20 * Math.log10(Math.max(v / m, 1e-4));
          return h - 14 - clamp((db + 48) / 48, 0, 1) * (h - 20);
        };
        // 横 = 何倍か(1〜40)。整数倍は太い棒、半整数倍(7.5 倍など)は細い棒。棒の真ん中がその倍数
        const cx = (hm: number) => (hm - 0.5) * bw;
        g.strokeStyle = "#262626";
        g.fillStyle = css("--faint");
        g.font = "9.5px sans-serif";
        const dbY = (db: number) => h - 14 - ((db + 48) / 48) * (h - 20);
        [0, -12, -24, -36].forEach((db) => {
          const y = dbY(db);
          g.beginPath();
          g.moveTo(0, y);
          g.lineTo(w, y);
          g.stroke();
        });
        am.forEach((a: number, j: number) => {
          const hm = (j + 1) / 2,
            whole = Number.isInteger(hm),
            bwid = whole ? bw - 2 : Math.max(2, bw * 0.3);
          if (hm > K) return;
          const audible = f0 * hm < 20000,
            audibleHi = f1 * hm < 20000,
            hov = FM4.hoverH === j;
          // 音域の高い端で鳴らない倍音は縞
          g.fillStyle = !audible ? "#3a3a3a" : hov ? "#6fb3ad" : !audibleHi ? stripe(g, whole ? "#4a8f8a" : css("--warn")) : whole ? "#4a8f8a" : css("--warn");
          const y = yOf(a);
          g.fillRect(cx(hm) - bwid / 2, y, bwid, h - 14 - y);
          if (am0) {
            const y0 = yOf(am0[j]);
            g.strokeStyle = "rgba(230,230,230,0.5)";
            g.setLineDash([2, 2]);
            g.strokeRect(cx(hm) - bwid / 2 + 0.5, y0, bwid - 1, h - 14 - y0);
            g.setLineDash([]);
          }
        });
        g.fillStyle = css("--faint");
        g.font = "10px sans-serif";
        axisRow(g, [1, 10, 20, 30, 40].map((k) => [String(k), cx(k) - 3] as [string, number]), h - 2, w);
        // dB の目盛りの字は棒の上に、暗い下地つきで(棒に隠れないように)。低い絵では間引く。いちばん上の字は絵の上で切れないよう下げる
        let lastY = -1e9;
        g.font = "9.5px sans-serif";
        [0, -12, -24, -36].forEach((db) => {
          const y = dbY(db),
            ty = Math.max(y - 2, 11);
          if (ty - lastY < 13) return;
          lastY = ty;
          const txt = `${db} dB`,
            tw = g.measureText(txt).width;
          g.fillStyle = "rgba(0,0,0,0.6)";
          g.fillRect(1, ty - 9, tw + 4, 11);
          g.fillStyle = css("--faint");
          g.fillText(txt, 3, ty);
        });
        if (si) {
          const hv = FM4.hoverH;
          si.textContent =
            hv != null
              ? tr(`${(hv + 1) / 2} 倍 · ${hzRange((f0 * (hv + 1)) / 2, (f1 * (hv + 1)) / 2)} · ${(20 * Math.log10(Math.max(am[hv] / m, 1e-4))).toFixed(0)} dB(いちばん強い所 = 0)`, `${(hv + 1) / 2}× · ${hzRange((f0 * (hv + 1)) / 2, (f1 * (hv + 1)) / 2)} · ${(20 * Math.log10(Math.max(am[hv] / m, 1e-4))).toFixed(0)} dB (strongest = 0)`)
              : tr("棒にマウスを置くと、倍数・Hz・強さ", "Hover a bar for its multiple, Hz and level");
        }
      }
      // つまんで倍率を動かしている間も、出ている色だけの凡例に
      refreshLegend("fmSpecLegend", FM4.specLegend(d));
    }
    ui.panes.forEach((p) => p.draw());
  },
  refresh() {
    rerender();
  },
  help: (): [string, string][] => [
    [tr("始めの音", "Starting sound"), tr("エレピ・ベル・ベースなど。選ぶと変調の構成と 4 つの箱(出力・変調)がその音になる", "E-piano, bell, bass… Picking one sets the layout and all four boxes (outputs / modulators)")],
    [
      tr("変調の構成", "Layout"),
      tr(
        "左で 8 つから選ぶ(エンジンが持つ決まった 8 つ)。水色の枠 = 出力(出口へ出す)、青い枠 = 変調(音色を作る)。矢印の太さ = どれだけ強く変えるか。見出しの「仕組み」にマウスを置くと絵で説明",
        "Pick one of the engine's 8 layouts on the left. Teal frame = output (to the out), blue frame = modulator (shapes the tone). Arrow width = how strongly it modulates. Hover “How it works” for a picture",
      ),
    ],
    [tr("出力・変調を調整", "Edit an operator"), tr("図の箱をクリック → 白く囲まれ、下の枠で調整できる", "Click a box in the diagram → it's outlined in white and editable below")],
    [tr("高さの倍率", "Ratio"), tr("鍵盤の高さの何倍で鳴らすか(周波数比)。「左右にずらす」で ▲ を動かす(0.5 ずつ。Alt で細かく)。整数倍は楽器らしく、非整数倍は鐘・金属の音", "How many times the key's pitch (frequency ratio). Use “Slide” to move ▲ (by 0.5; Alt for fine). Whole numbers sound instrument-like; others like bells or metal")],
    [tr("音量 / 変調の強さの変わり方", "Level / mod depth over time"), tr("箱ごとの、押してから離すまでの変わり方。出力は音量、変調は変調の強さ(= 倍音の量)の変わり方になる", "Each box's change from press to release: level for outputs, modulation depth (= amount of harmonics) for modulators")],
    [tr("出口の音の倍音", "Harmonics at the out"), tr("鳴り始めと 0.5 秒後を切り替えて比べる(点線 = 鳴り始め)。エンジンで実際に鳴らして測った値", "Switch between the start and 0.5 s later (dotted = start). Measured by actually playing the note in the engine")],
  ],
};

register(FM4);
export default FM4;
