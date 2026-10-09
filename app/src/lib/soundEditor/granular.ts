// 音色エディタ: グラニュラー(素材から短い粒を切り出して重ねる)。
// 左 = 素材(曲の中の音声クリップ・使っている素材・ファイル)、上 = 素材の波形(取り出す位置・ばらつき・進み方)、
// 下 = 粒 1 つの音量の形・粒の散らばり(横 = 時間、縦 = 音程、下の帯 = 左右)・全体の音量の変わり方、右 = つまみ
import { open as pickFile } from "@tauri-apps/plugin-dialog";
import * as api from "../api";
import { tr } from "../i18n.svelte";
import { showToast } from "../toast.svelte";
import { draw as drawAll, register, type Data, type Loaded } from "./core.svelte";
import { axisRow, byId, clamp, css, ctx2d, drawAudio, el, ico, noteName, seeded, tag, timeRuler } from "./dom";
import {
  canEdit,
  cur,
  envMax,
  envPane,
  explainOn,
  knob,
  knobOf,
  legendRow,
  pane,
  pushHist,
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
} from "./parts";

const GRAN_WINDOWS = (): [string, string, string][] => [
  ["hann", tr("なめらか", "Smooth"), tr("柔らかくつながる(雲・パッド)", "Blends softly (clouds, pads)")],
  ["triangle", tr("三角", "Triangle"), tr("少し輪郭が出る", "A little more defined")],
  ["trapezoid", tr("台形", "Trapezoid"), tr("粒の真ん中が長く、元の音が分かりやすい", "Long middle; the source is easier to recognize")],
  ["perc", tr("鋭い", "Sharp"), tr("頭が立って減っていく(ぱらぱら・ちりちり)", "Sharp start, then decays (crackly, sparkly)")],
];
/** 粒 1 つの音量の形(エンジンの GrainWindow と同じ 4 つ) */
const winAt = (k: string, t: number) =>
  k === "hann" ? Math.sin(Math.PI * t) ** 2 : k === "triangle" ? 1 - Math.abs(2 * t - 1) : k === "trapezoid" ? Math.min(1, t * 6, (1 - t) * 6) : t < 0.03 ? t / 0.03 : (1 - (t - 0.03) / 0.97) ** 3;

/** 素材の波形(最小・最大を交互に)と長さ。素材の ID ごと */
const waves: Record<string, { x: number[]; secs: number } | "loading"> = {};
/** 取り出す位置からの 0.5 秒の細かい波形(粒 1 つの絵) */
let slice: { key: string; x: number[] } | null = null;
let sliceKey = "";

const GRAN: any = {
  kind: "granular",
  title: () => tr("グラニュラー", "Granular"),
  load({ params }: Loaded): Data {
    const n = (k: string, def: number) => (typeof params[k] === "number" ? params[k] : def);
    return {
      src: typeof params.sample === "string" ? params.sample : "",
      position: n("position", 0.3),
      spray: n("spray_ms", 40),
      grain: n("grain_ms", 90),
      density: n("density", 30),
      pitchRand: n("pitch_rand", 0.1),
      spread: n("spread", 0.5),
      window: typeof params.window === "string" ? params.window : "hann",
      scan: n("scan", 0),
      root: Math.round(n("root", 60)),
      a: n("attack", 0.2),
      d: n("decay", 1),
      s: n("sustain", 1),
      r: n("release", 1),
      gain: n("gain_db", 0),
    };
  },
  uiInit: (): Data => ({ showShelf: false }),
  params: (d: Data) => ({
    sample: d.src,
    position: d.position,
    spray_ms: d.spray,
    grain_ms: d.grain,
    density: d.density,
    pitch_rand: d.pitchRand,
    spread: d.spread,
    window: d.window,
    scan: d.scan,
    root: d.root,
    attack: d.a,
    decay: d.d,
    sustain: d.s,
    release: d.r,
    gain_db: d.gain,
  }),
  grains: [] as { t0: number; x: number; y: number }[],
  hoverX: null as number | null,
  hoverOne: null as number | null,
  hoverCloud: null as [number, number] | null,
  hoverPan: null as number | null,
  /** 素材の候補: 曲の中の音声クリップ・ほかのトラックが使っている素材・今の素材 */
  sources(): { id: string; name: string; desc: string; secs: number }[] {
    const p = state.project;
    if (!p) return [];
    const out = new Map<string, { id: string; name: string; desc: string; secs: number }>();
    const secsOf = (id: string) => {
      const a = (p.assets as any)?.[id];
      return a ? a.frames / Math.max(1, a.sample_rate) : 0;
    };
    for (const t of p.tracks)
      for (const c of t.clips as any[]) {
        if (c.kind !== "audio" || !c.asset || out.has(c.asset)) continue;
        const s = secsOf(c.asset);
        out.set(c.asset, { id: c.asset, name: c.name || t.name, desc: tr(`曲の中の音声クリップ · ${s.toFixed(1)} 秒`, `Audio clip in the song · ${s.toFixed(1)} s`), secs: s });
      }
    for (const t of p.tracks) {
      const dv: any = t.device;
      const id = dv?.type === "sampler" ? dv.asset : dv?.name === "granular" ? dv.params?.sample : null;
      if (typeof id !== "string" || !id || out.has(id)) continue;
      const s = secsOf(id);
      out.set(id, { id, name: t.name, desc: tr(`「${t.name}」の素材 · ${s.toFixed(1)} 秒`, `Source of "${t.name}" · ${s.toFixed(1)} s`), secs: s });
    }
    return [...out.values()];
  },
  /** 素材の波形(無ければ取りに行き、届いたら描き直す) */
  wave(id: string): number[] | null {
    if (!id) return null;
    const w = waves[id];
    if (w && w !== "loading") return w.x;
    if (!w) {
      waves[id] = "loading";
      api
        .assetPeaks(id, 1500)
        .then((r) => {
          waves[id] = { x: r.peaks, secs: r.seconds };
          rerender();
        })
        .catch(() => delete waves[id]);
    }
    return null;
  },
  len(d: Data) {
    const w = waves[d.src];
    if (w && w !== "loading") return Math.max(0.01, w.secs);
    const s = GRAN.sources().find((x: any) => x.id === d.src);
    return Math.max(0.01, s?.secs || 1);
  },
  /** 取り出す位置から 0.5 秒の細かい波形(粒 1 つの絵)。無ければ取りに行く */
  slice(d: Data): number[] | null {
    if (!d.src) return null;
    const start = d.position * GRAN.len(d);
    const key = `${d.src}:${start.toFixed(3)}`;
    if (slice?.key === key) return slice.x;
    if (sliceKey !== key) {
      sliceKey = key;
      api
        .assetPeaks(d.src, 1000, [start, start + 0.5])
        .then((r) => {
          if (sliceKey !== key) return;
          // 最小・最大の真ん中を、その所の値として描く
          const x: number[] = [];
          for (let i = 0; i + 1 < r.peaks.length; i += 2) x.push((r.peaks[i] + r.peaks[i + 1]) / 2);
          slice = { key, x };
          GRAN.draw();
        })
        .catch(() => (sliceKey = ""));
    }
    return slice?.x ?? null;
  },
  render(body: HTMLElement) {
    const d = cur();
    body.innerHTML = "";
    // ---- 左: 素材 ----
    const left = el("div", { class: "col left" });
    const srcSeg = el("div", { class: "seg" });
    (
      [
        ["src", tr("素材", "Sources")],
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
    const clips = el("div", { class: "cliplist" });
    const srcs = GRAN.sources();
    if (!srcs.length) clips.append(el("div", { class: "hint" }, tr("曲の中に音声がありません。下の「ファイルから選ぶ…」で読み込めます", "No audio in the song yet. Load one with “Choose a file…” below")));
    srcs.forEach((s: any) => {
      const c = el("canvas");
      clips.append(
        el(
          "div",
          {
            class: "clipitem" + (d.src === s.id ? " on" : ""),
            onclick: () => {
              if (!canEdit()) return;
              d.src = s.id;
              pushHist(tr(`素材を「${s.name}」に`, `Source → "${s.name}"`));
              rerender();
            },
          },
          s.name,
          el("small", { class: "dim", style: "display:block" }, s.desc),
          c,
        ),
      );
      requestAnimationFrame(() => {
        const x = GRAN.wave(s.id);
        if (!x || !c.isConnected) return;
        const { g, w, h } = ctx2d(c);
        drawAudio(g, x, w, 0, h);
      });
    });
    const pickBtn = el(
      "button",
      {
        class: "btn",
        onclick: async () => {
          if (!canEdit()) return;
          const file = await pickFile({
            title: tr("粒を取り出す音声を選ぶ", "Choose audio to take grains from"),
            filters: [{ name: tr("音声(WAV / MP3 / FLAC / OGG / M4A)", "Audio (WAV / MP3 / FLAC / OGG / M4A)"), extensions: ["wav", "mp3", "flac", "ogg", "m4a", "aac"] }],
          });
          if (typeof file !== "string") return;
          try {
            await api.importSample(state.track, file, "granular");
          } catch (e) {
            showToast("error", tr(`読み込めませんでした: ${e}`, `Couldn't load it: ${e}`));
          }
        },
      },
      tr("ファイルから選ぶ…", "Choose a file…"),
    );
    left.append(
      el(
        "div",
        { class: "box" },
        el("h3", {}, tr("素材", "Source"), el("span", { class: "spacer" }), el("span", { class: "hint" }, tr("粒を取り出す音声", "Audio to take grains from"))),
        srcSeg,
        d.showShelf ? shelfList("granular") : clips,
        d.showShelf ? null : pickBtn,
      ),
    );
    // ---- 真ん中 ----
    // 低い画面では真ん中の列ごとスクロール。下は散らばりの絵・左右の帯・つまみ(2 段)まで入る高さ
    const center = el("div", { class: "col center wtcol", style: "overflow-y: auto" });
    const grid = el("div", { class: "wtgrid", style: "grid-template-rows: minmax(190px, 1fr) minmax(330px, 1.25fr)" });
    const tool = toolOf("wave");
    const wc = el("canvas", { id: "grWave", class: "fill" });
    const winfo = el("div", { class: "paneinfo", id: "grInfo" });
    const srcName = srcs.find((s: any) => s.id === d.src)?.name ?? (d.src ? d.src.slice(0, 18) : tr("(なし)", "(none)"));
    grid.append(
      el(
        "div",
        { class: "box", style: "grid-column: span 4" },
        el("h3", {}, tr(`素材の波形: ${srcName}`, `Source waveform: ${srcName}`), el("span", { class: "spacer" }), GRAN.howBtn()),
        toolSeg(
          [
            VIEW_TOOL(),
            ["pos", tr("取り出す位置を動かす", "Move the position"), tr("白い線(取り出す位置)をつまんで左右に", "Drag the white line (position) left/right"), "position"],
            ["spray", tr("ばらつきを広げる", "Widen the spray"), tr("網掛けの端をつまんで、粒を取り出す範囲を広げる・狭める", "Drag the edge of the hatched area to widen or narrow where grains come from"), "spread"],
            [
              "scan",
              tr("進み方を決める", "Set the scan"),
              tr("白い線から左右へドラッグして、押さえている間に取り出す位置が進む向きと速さを決める(矢印の先 = 押さえて 1 秒後の位置)", "Drag from the white line to set which way and how fast the position moves while a key is held (arrow tip = position after 1 s)"),
              "scan",
            ],
          ],
          tool,
          (t) => setTool("wave", t),
        ),
        wc,
        winfo,
        legendRow([
          ["now", tr("粒を取り出す位置", "Grain position")],
          ["spray", tr("ばらつく範囲", "Spray range")],
          ["grain", tr("鳴っている粒", "Sounding grains")],
          [el("span", { class: "sw arrow" }), tr("進む向きと速さ", "Scan direction and speed"), tr("取り出す位置が進む。矢印の先 = 押さえて 1 秒後に取り出す位置", "The position moves; arrow tip = where grains come from after holding 1 s")],
        ]),
      ),
    );
    grid.append(specBox("grid-column: span 2"));
    // 下: 粒
    const one = el("canvas", { id: "grOne", class: "fill" });
    const cloud = el("canvas", { id: "grCloud", class: "fill" });
    const panC = el("canvas", { id: "grPan", style: "height: 22px; flex: none", title: tr("粒をステレオの左右(パン。L = 左・R = 右)のどこに置くか(左右の広がり)。点 1 つ = 粒 1 つ", "Where grains sit in stereo (pan; L = left, R = right): the spread. One dot = one grain") });
    // マウスで文が変わるので 2 行分を取っておく(下のつまみが押し出されないように)
    const oinfo = el("span", { class: "hint", id: "grOneInfo", style: "min-height: 2lh" }),
      cinfo = el("span", { class: "hint", id: "grCloudInfo", style: "min-height: 2lh" });
    const winSeg = el("div", { class: "seg grid2" });
    GRAN_WINDOWS().forEach(([k, n, desc]) =>
      winSeg.append(
        el(
          "button",
          {
            class: d.window === k ? "on" : "",
            title: desc,
            onclick: () => {
              if (!canEdit()) return;
              d.window = k;
              pushHist(tr(`粒 1 つの音量の形を「${n}」に`, `Grain shape → "${n}"`));
              rerender();
            },
          },
          ico("w_" + k),
          n,
        ),
      ),
    );
    grid.append(
      el(
        "div",
        { class: "box editgroup", style: "grid-column: span 6" },
        el(
          "div",
          { class: "edithead" },
          el("b", {}, tr("音の粒の調整", "Grains")),
          el("span", { class: "spacer" }),
          el(
            "span",
            { class: "hint" },
            tr(
              `1 秒に ${d.density} 粒 · 1 粒 ${d.grain} ms · 同時に ${Math.max(1, Math.round((d.density * d.grain) / 1000))} 粒ほど重なる`,
              `${d.density} grains/s · ${d.grain} ms each · about ${Math.max(1, Math.round((d.density * d.grain) / 1000))} overlap`,
            ),
          ),
        ),
        el(
          "div",
          { class: "editpanes three" },
          // 道具の段を先にして、ほかの欄の「見るだけ」とそろえる
          pane(tr("粒 1 つの音量の形", "One grain's shape"), {
            info: oinfo,
            tools: el(
              "div",
              { class: "toolstack" },
              toolSeg([VIEW_TOOL(), ["points", tr("長さをつまむ", "Drag the length"), tr("右端の ○ をつまんで、粒の長さを変える", "Drag the ○ at the right end to change the grain length")]], toolOf("one"), (t) => setTool("one", t)),
              el("div", { class: "toolseg" }, winSeg),
            ),
            canvas: one,
          }),
          pane(tr("粒の散らばり", "Grain scatter"), {
            info: cinfo,
            head: el("span", { class: "small dim", title: tr("上の絵: 横 = 取り出す位置のずれ(時間)、縦 = 音程のずれ(高音・低音)。下の帯: ステレオの左右(パン)", "Top: across = position offset (time), up/down = pitch offset (high/low). Bottom strip: stereo left/right (pan)") }, tr("点 1 つ = 粒 1 つ", "1 dot = 1 grain")),
            tools: toolSeg(
              [
                VIEW_TOOL(),
                [
                  "points",
                  tr("角をつまむ", "Drag the corner"),
                  tr("右上の ○ をつまむ: 横 = 位置のばらつき、縦 = 音程のばらつき。下の帯の端の ○ をつまむ: 左右の広がり", "Drag the top-right ○: across = spray, up/down = pitch randomness. Drag the ○ at the strip's ends: spread"),
                ],
              ],
              toolOf("cloud"),
              (t) => setTool("cloud", t),
            ),
            canvas: cloud,
            below: panC,
          }),
          envPane("grEnv", { title: tr("全体の音量の変わり方", "Overall level over time"), get: () => shown() as any, set: (k, v) => (cur()[k] = v), max: { a: 10, d: 10, r: 10 } }),
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
        el("h3", {}, tr("音の粒", "Grains")),
        k("取り出す位置", "position", 0, 1, 0.001, "", { fmt: (v: number) => `${Math.round(v * 100)}%` }),
        k("位置のばらつき", "spray", 0, 2000, 10, " ms"),
        k("粒の長さ", "grain", 5, 500, 1, " ms"),
        k("粒の数", "density", 1, 200, 1, tr(" /秒", " /s")),
        k("音程のばらつき", "pitchRand", 0, 12, 0.1, tr(" 半音", " st")),
        k("左右の広がり", "spread", 0, 1, 0.01),
        k("進む速さ", "scan", -2, 2, 0.01, "", { fmt: (v: number) => `×${v.toFixed(2)}` }),
      ),
    );
    right.append(
      el(
        "div",
        { class: "box" },
        el("h3", {}, tr("鳴らし方", "Playing")),
        knob("録音どおりの鍵盤", {
          min: 0,
          max: 127,
          step: 1,
          value: d.root,
          fmt: (v) => noteName(v),
          title: tr("この鍵盤で弾くと、粒が録音どおりの高さで鳴る(元の高さ・ルート)", "Playing this key plays grains at the recording's own pitch (root)"),
          onInput: (v) => {
            d.root = v;
            drawAll();
          },
          onCommit: (v) => pushHist(tr(`録音どおりの鍵盤を ${noteName(v)} に`, `Root key → ${noteName(v)}`)),
        }),
        k("ゲイン", "gain", -24, 12, 0.5, " dB"),
      ),
    );
    right.append(
      el(
        "div",
        { class: "box" },
        el("h3", {}, tr("全体の音量の変わり方", "Overall level over time"), el("span", { class: "spacer" }), el("span", { class: "hint" }, tr("押す → 離す", "press → release"))),
        k("立ち上がり", "a", 0.001, envMax("grEnv", 10), 0.001, " s"),
        k("下がる時間", "d", 0.01, envMax("grEnv", 10), 0.01, " s"),
        k("保つ音量", "s", 0, 1, 0.01),
        k("余韻", "r", 0.01, envMax("grEnv", 10), 0.01, " s"),
      ),
    );
    right.append(saveBox(state.trackName, tr("素材と粒の設定", "the source and grain settings")));
    body.append(left, center, right);
    // ---- 素材の波形の操作 ----
    let drag: string | null = null;
    const fxOf = (ev: PointerEvent): [number, number] => {
      const r = wc.getBoundingClientRect();
      return [clamp((ev.clientX - r.left) / r.width, 0, 1), r.width];
    };
    const sprF = () => d.spray / 1000 / GRAN.len(d);
    const grab = (fx: number, w: number) =>
      tool === "pos" || tool === "scan" ? (Math.abs(fx - d.position) * w < 10 ? tool : null) : tool === "spray" ? (Math.abs(Math.abs(fx - d.position) - sprF()) * w < 10 ? "spray" : null) : null;
    wc.addEventListener("pointerdown", (ev) => {
      if (tool === "view" || !canEdit()) return;
      const [fx, w] = fxOf(ev);
      drag = grab(fx, w) ?? (tool === "pos" ? "pos" : null);
      if (drag) {
        wc.setPointerCapture(ev.pointerId);
        if (drag === "pos") {
          d.position = fx;
          drawAll();
        }
      }
    });
    wc.addEventListener("pointermove", (ev) => {
      const [fx, w] = fxOf(ev);
      GRAN.hoverX = fx;
      if (!drag) {
        wc.style.cursor = grab(fx, w) ? "ew-resize" : tool === "pos" ? "pointer" : "default";
        GRAN.draw();
        return;
      }
      if (drag === "pos") d.position = fx;
      else if (drag === "spray") d.spray = clamp(Math.round((Math.abs(fx - d.position) * GRAN.len(d) * 1000) / 10) * 10, 0, 2000);
      // 矢印の先 = 1 秒後の位置
      else d.scan = clamp(Math.round((fx - d.position) * GRAN.len(d) * 100) / 100, -2, 2);
      drawAll();
    });
    wc.addEventListener("pointerup", () => {
      if (!drag) return;
      pushHist(
        ({
          pos: tr(`取り出す位置を ${Math.round(d.position * 100)}% に`, `Position → ${Math.round(d.position * 100)}%`),
          spray: tr(`位置のばらつきを ${d.spray} ms に`, `Spray → ${d.spray} ms`),
          scan: tr(`進む速さを ×${d.scan.toFixed(2)} に`, `Scan → ×${d.scan.toFixed(2)}`),
        } as Record<string, string>)[drag],
      );
      drag = null;
      rerender();
    });
    wc.addEventListener("pointerleave", () => {
      GRAN.hoverX = null;
      GRAN.draw();
    });
    // ---- 1 つの粒: 右端の ○ で長さ ----
    let odrag = false;
    // つまむ道具のときは手のカーソル
    const grabCur = (c: HTMLElement, on: boolean, dragging: boolean) => {
      c.style.cursor = on ? (dragging ? "grabbing" : "grab") : "default";
    };
    one.addEventListener("pointerdown", (ev) => {
      if (toolOf("one") !== "points" || !canEdit()) return;
      odrag = true;
      one.setPointerCapture(ev.pointerId);
      grabCur(one, true, true);
    });
    one.addEventListener("pointermove", (ev) => {
      const r = one.getBoundingClientRect(),
        f = clamp((ev.clientX - r.left - 10) / (r.width - 20), 0, 1);
      GRAN.hoverOne = f;
      grabCur(one, toolOf("one") === "points", odrag);
      if (odrag) {
        d.grain = clamp(Math.round(f * 500), 5, 500);
        drawAll();
      } else GRAN.draw();
    });
    one.addEventListener("pointerleave", () => {
      GRAN.hoverOne = null;
      GRAN.draw();
    });
    one.addEventListener("pointerup", () => {
      if (!odrag) return;
      odrag = false;
      pushHist(tr(`粒の長さを ${d.grain} ms に`, `Grain length → ${d.grain} ms`));
      rerender();
    });
    // ---- 粒の散らばり: 右上の ○ で位置と音程のばらつき ----
    let cdrag = false;
    cloud.addEventListener("pointerdown", (ev) => {
      if (toolOf("cloud") !== "points" || !canEdit()) return;
      cdrag = true;
      cloud.setPointerCapture(ev.pointerId);
      grabCur(cloud, true, true);
    });
    cloud.addEventListener("pointerleave", () => {
      GRAN.hoverCloud = null;
      GRAN.draw();
    });
    cloud.addEventListener("pointermove", (ev) => {
      const r = cloud.getBoundingClientRect(),
        G = GRAN.cloudLayout(r.width, r.height);
      GRAN.hoverCloud = [ev.clientX - r.left, ev.clientY - r.top];
      grabCur(cloud, toolOf("cloud") === "points", cdrag);
      if (!cdrag) {
        GRAN.draw();
        return;
      }
      d.spray = clamp(Math.round((2000 * Math.pow(clamp((ev.clientX - r.left - G.cx) / G.rx, 0, 1), 2)) / 10) * 10, 0, 2000);
      d.pitchRand = clamp(Math.round(12 * Math.pow(clamp((G.cy - (ev.clientY - r.top)) / G.ry, 0, 1), 2) * 10) / 10, 0, 12);
      drawAll();
    });
    // 左右の帯: 「角をつまむ」のとき、帯の端の ○ をつまんで左右の広がりを変える
    let pdrag = false;
    const panAt = (ev: PointerEvent) => {
      const r = panC.getBoundingClientRect(),
        half = r.width / 2 - 24;
      return clamp(Math.round((Math.abs(ev.clientX - r.left - r.width / 2) / half) * 100) / 100, 0, 1);
    };
    panC.addEventListener("pointerdown", (ev) => {
      if (toolOf("cloud") !== "points" || !canEdit()) return;
      pdrag = true;
      panC.setPointerCapture(ev.pointerId);
      grabCur(panC, true, true);
      d.spread = panAt(ev);
      drawAll();
    });
    panC.addEventListener("pointermove", (ev) => {
      grabCur(panC, toolOf("cloud") === "points", pdrag);
      GRAN.hoverPan = panAt(ev);
      if (pdrag) {
        d.spread = GRAN.hoverPan;
        drawAll();
      } else GRAN.draw();
    });
    panC.addEventListener("pointerleave", () => {
      GRAN.hoverPan = null;
      GRAN.draw();
    });
    panC.addEventListener("pointerup", () => {
      if (!pdrag) return;
      pdrag = false;
      pushHist(tr(`左右の広がりを ${d.spread} に`, `Spread → ${d.spread}`));
      rerender();
    });
    cloud.addEventListener("pointerup", () => {
      if (!cdrag) return;
      cdrag = false;
      pushHist(tr(`位置のばらつきを ±${d.spray} ms・音程のばらつきを ±${d.pitchRand} 半音 に`, `Spray → ±${d.spray} ms · pitch randomness → ±${d.pitchRand} st`));
      rerender();
    });
  },
  /** 「仕組み」: マウスを置くと、素材から短い粒を切り出して重ねる様子を小さな絵で見せる */
  howBtn() {
    const b = el("span", { class: "howbtn" }, ico("info"), tr("仕組み", "How it works"));
    queueMicrotask(() =>
      explainOn(b, "grhow", (pop) => {
        const c = el("canvas", { style: "height:150px" });
        pop.append(
          el("b", {}, tr("粒(グレイン)とは", "What a grain is")),
          c,
          el(
            "div",
            { class: "small" },
            tr(
              "素材の白い線の所から、とても短い音(粒。ふつう 20〜200 ms)を次々に切り出し、少しずつずらして重ねて鳴らします。1 粒は短すぎて元の音には聞こえず、たくさん重なると雲のような音になります。取り出す位置を少しずつ進める(進む速さ)と、素材を引き伸ばしたように聞こえます。逆向きに進めても、粒 1 つ 1 つは順向きに鳴ります。",
              "Very short sounds (grains, usually 20–200 ms) are cut one after another from the white line in the source and played overlapping, slightly offset. A single grain is too short to sound like the source; many together sound like a cloud. Moving the position slowly (scan) sounds like stretching the source. Even scanning backwards, each grain plays forward.",
            ),
          ),
        );
        requestAnimationFrame(() => {
          const { g, w } = ctx2d(c);
          const x = GRAN.wave(cur()?.src ?? "") ?? Array.from({ length: 600 }, (_, i) => Math.sin(i * 0.2) * 0.5 * (0.5 + 0.5 * Math.sin(i / 60)));
          g.font = "10.5px sans-serif";
          g.fillStyle = css("--dim");
          g.fillText(tr("素材(白い線の所から切り出す)", "Source (cut from the white line)"), 4, 11);
          drawAudio(g, x, w, 16, 58);
          const px = w * 0.42;
          g.fillStyle = "#fff";
          g.fillRect(px - 1, 14, 2, 46);
          g.fillStyle = css("--dim");
          g.fillText(tr("粒を少しずつずらして重ねる", "Grains overlap, slightly offset"), 4, 80);
          for (let i = 0; i < 7; i++) {
            const gx = 10 + i * ((w - 80) / 6),
              gy = 90 + (i % 3) * 18,
              gw = 60;
            g.strokeStyle = "rgba(232,196,106,0.9)";
            g.beginPath();
            g.moveTo(gx, gy + 16);
            for (let k = 0; k <= 30; k++) {
              const t = k / 30;
              g.lineTo(gx + t * gw, gy + 16 - Math.sin(Math.PI * t) ** 2 * 14);
            }
            g.stroke();
            g.strokeStyle = "rgba(232,196,106,0.35)";
            g.beginPath();
            g.moveTo(px, 60);
            g.lineTo(gx + gw / 2, gy);
            g.stroke();
          }
        });
      }),
    );
    return b;
  },
  cloudLayout(w: number, h: number) {
    const rx = w / 2 - 14,
      ry = h / 2 - 14;
    return { cx: w / 2, cy: h / 2, rx, ry, xOf: (ms: number) => Math.sqrt(ms / 2000) * rx, yOf: (st: number) => Math.sqrt(st / 12) * ry };
  },
  draw() {
    const d = shown();
    if (!d) return;
    const c = byId<HTMLCanvasElement>("grWave");
    if (!c) return;
    const { g, w, h } = ctx2d(c);
    const x = GRAN.wave(d.src),
      len = GRAN.len(d),
      top = 18,
      bot = h - 30,
      tool = toolOf("wave");
    timeRuler(g, 0, w, h - 15, len, tr(" 秒", " s"));
    const pos = d.position * w,
      spr = (d.spray / 1000 / len) * w;
    // ばらつく範囲(斜線)
    g.save();
    g.beginPath();
    g.rect(pos - spr, top, spr * 2, bot - top);
    g.clip();
    g.strokeStyle = "rgba(37,189,177,0.28)";
    for (let i = -h; i < w; i += 7) {
      g.beginPath();
      g.moveTo(i, bot);
      g.lineTo(i + h, top - (h - bot));
      g.stroke();
    }
    g.restore();
    if (tool === "spray") {
      g.fillStyle = css("--human");
      [pos - spr, pos + spr].forEach((xx) => g.fillRect(xx - 1.5, top, 3, bot - top));
    }
    if (x) drawAudio(g, x, w, top, bot);
    else {
      g.fillStyle = css("--faint");
      g.font = "11px sans-serif";
      g.fillText(d.src ? tr("素材を読み込んでいます…", "Loading the source…") : tr("素材がありません(左で選ぶか、ファイルから選ぶ)", "No source (pick one on the left or choose a file)"), 8, (top + bot) / 2);
    }
    // 鳴っている粒(鍵盤を押している間。エンジンと同じ数え方で描く)
    const now = performance.now();
    GRAN.grains = GRAN.grains.filter((gr: any) => now - gr.t0 < d.grain);
    GRAN.grains.forEach((gr: any) => {
      const age = (now - gr.t0) / d.grain;
      g.fillStyle = `rgba(232,196,106,${0.85 * (1 - age)})`;
      g.fillRect(gr.x * w, (top + bot) / 2 - 18 + gr.y, Math.max(3, (d.grain / 1000 / len) * w), 36);
    });
    // 位置(白)と進む速さの矢印。矢印の先 = 押さえて 1 秒後の位置(時間の物差しと同じ尺度)
    g.fillStyle = "#fff";
    g.fillRect(pos - 1, 4, 2, h - 8);
    if (d.scan) {
      const ex = pos + (d.scan / len) * w;
      g.strokeStyle = css("--accent");
      g.lineWidth = 2;
      g.beginPath();
      g.moveTo(pos, 10);
      g.lineTo(ex, 10);
      g.stroke();
      g.fillStyle = css("--accent");
      g.beginPath();
      g.moveTo(ex, 10);
      g.lineTo(ex - Math.sign(d.scan) * 7, 5);
      g.lineTo(ex - Math.sign(d.scan) * 7, 15);
      g.fill();
      g.lineWidth = 1;
      tag(g, tr(`進む速さ ×${d.scan.toFixed(2)}`, `Scan ×${d.scan.toFixed(2)}`), Math.max(pos, ex) + 6, 14, w, css("--accent"));
    }
    tag(g, tr(`取り出す位置 ${Math.round(d.position * 100)}%(${(d.position * len).toFixed(2)} 秒)`, `Position ${Math.round(d.position * 100)}% (${(d.position * len).toFixed(2)} s)`), pos + 6, bot - 4, w);
    const info = byId("grInfo");
    if (info)
      info.textContent =
        GRAN.hoverX != null
          ? tr(`${(GRAN.hoverX * len).toFixed(2)} 秒(${Math.round(GRAN.hoverX * 100)}%)`, `${(GRAN.hoverX * len).toFixed(2)} s (${Math.round(GRAN.hoverX * 100)}%)`)
          : tr(
              `${len.toFixed(1)} 秒 · 位置のばらつき ±${d.spray} ms · 進む向き ${d.scan > 0 ? "→" : d.scan < 0 ? "←" : "なし(止まったまま)"}`,
              `${len.toFixed(1)} s · spray ±${d.spray} ms · scan ${d.scan > 0 ? "→" : d.scan < 0 ? "←" : "none (frozen)"}`,
            );
    // 1 つの粒
    const oc = byId<HTMLCanvasElement>("grOne");
    if (oc) {
      const { g: g2, w: w2, h: h2 } = ctx2d(oc);
      const x0 = 10,
        xw = w2 - 20,
        gx = x0 + (d.grain / 500) * xw,
        t2 = 14,
        b2 = h2 - 16;
      g2.strokeStyle = "#262626";
      g2.fillStyle = css("--faint");
      g2.font = "10px sans-serif";
      [0, 100, 200, 300, 400, 500].forEach((ms) => {
        const xx = x0 + (ms / 500) * xw;
        g2.beginPath();
        g2.moveTo(xx, t2);
        g2.lineTo(xx, b2);
        g2.stroke();
      });
      axisRow(g2, [0, 100, 200, 300, 400, 500].map((ms) => [ms === 500 ? "500 ms" : `${ms}`, x0 + (ms / 500) * xw - 6] as [string, number]), h2 - 3, w2);
      if (GRAN.hoverOne != null) {
        const xx = x0 + GRAN.hoverOne * xw;
        g2.strokeStyle = "rgba(255,255,255,0.25)";
        g2.beginPath();
        g2.moveTo(xx, t2);
        g2.lineTo(xx, b2);
        g2.stroke();
      }
      // 形(音量の付け方)の山
      g2.fillStyle = "rgba(37,189,177,0.10)";
      g2.strokeStyle = "rgba(37,189,177,0.55)";
      g2.lineWidth = 1.5;
      g2.setLineDash([4, 3]);
      g2.beginPath();
      g2.moveTo(x0, b2);
      for (let i = 0; i <= 60; i++) {
        const t = i / 60;
        g2.lineTo(x0 + t * (gx - x0), b2 - winAt(d.window, t) * (b2 - t2));
      }
      g2.lineTo(gx, b2);
      g2.fill();
      g2.stroke();
      g2.setLineDash([]);
      // 取り出した素材の切れ端(灰)と、形を掛けた実際の粒の波(水色)。真ん中の線を 0 にして上下に振る
      const mid2 = (t2 + b2) / 2,
        amp2 = (b2 - t2) / 2,
        sl = GRAN.slice(d);
      if (sl && sl.length) {
        const peak = Math.max(1e-3, ...sl.map((v: number) => Math.abs(v)));
        const sAt = (ms: number) => clamp(sl[clamp(Math.floor((ms / 500) * sl.length), 0, sl.length - 1)] / peak, -1, 1);
        ([["rgba(160,160,160,0.45)", false], [css("--accent"), true]] as [string, boolean][]).forEach(([col, shaped]) => {
          g2.strokeStyle = col;
          g2.lineWidth = shaped ? 1.6 : 1;
          g2.beginPath();
          for (let px = 0; px <= gx - x0; px++) {
            const ms = (px / xw) * 500,
              t = ms / d.grain,
              v = sAt(ms) * (shaped ? winAt(d.window, t) : 1);
            if (px) g2.lineTo(x0 + px, mid2 - v * amp2);
            else g2.moveTo(x0 + px, mid2 - v * amp2);
          }
          g2.stroke();
        });
      }
      if (toolOf("one") === "points") {
        g2.beginPath();
        g2.arc(gx, b2, 6, 0, Math.PI * 2);
        g2.fillStyle = css("--human");
        g2.fill();
      }
      const oi = byId("grOneInfo");
      if (oi) {
        const hm = GRAN.hoverOne == null ? null : Math.round(GRAN.hoverOne * 500);
        const wn = GRAN_WINDOWS().find((w0) => w0[0] === d.window)?.[1] ?? d.window;
        oi.textContent =
          hm != null
            ? hm <= d.grain
              ? tr(`${hm} ms · 音量 ${Math.round(winAt(d.window, hm / d.grain) * 100)}%`, `${hm} ms · level ${Math.round(winAt(d.window, hm / d.grain) * 100)}%`)
              : tr(`${hm} ms · 粒の外(鳴らない)`, `${hm} ms · outside the grain (silent)`)
            : tr(`${wn} · ${d.grain} ms(灰 = 切れ端・水色 = 粒)`, `${wn} · ${d.grain} ms (gray = the slice, teal = the grain)`);
      }
    }
    // 粒の散らばり: 横 = 取り出す位置のずれ(ms)、縦 = 音程のずれ(半音)。左右は下の帯
    const cc = byId<HTMLCanvasElement>("grCloud");
    if (cc) {
      const { g: g3, w: w3, h: h3 } = ctx2d(cc);
      const G = GRAN.cloudLayout(w3, h3);
      g3.strokeStyle = "#2a2a2a";
      g3.beginPath();
      g3.moveTo(0, G.cy);
      g3.lineTo(w3, G.cy);
      g3.moveTo(G.cx, 0);
      g3.lineTo(G.cx, h3);
      g3.stroke();
      const rx = G.xOf(d.spray),
        ry = G.yOf(d.pitchRand);
      g3.strokeStyle = "rgba(37,189,177,0.6)";
      g3.setLineDash([3, 3]);
      g3.strokeRect(G.cx - rx, G.cy - ry, rx * 2, ry * 2 || 1);
      g3.setLineDash([]);
      const rnd = seeded(7 + Math.round(d.density));
      // 目盛り(平方根の間隔): 横 = ±100・500・2000 ms、縦 = ±1・4・12 半音
      g3.fillStyle = css("--faint");
      g3.font = "9px sans-serif";
      g3.strokeStyle = "#3a3a3a";
      let lastX = -1e9;
      [100, 500, 2000].forEach((ms) =>
        [-1, 1].forEach((sg) => {
          const xx = G.cx + sg * G.xOf(ms);
          g3.beginPath();
          g3.moveTo(xx, G.cy - 3);
          g3.lineTo(xx, G.cy + 3);
          g3.stroke();
          if (sg > 0 && xx - lastX > 34) {
            g3.fillText(ms >= 1000 ? `${ms / 1000}s` : `${ms}ms`, xx - 10, G.cy + 13);
            lastX = xx;
          }
        }),
      );
      // 低い絵では字が重ならないよう間引く(下から上へ)
      let lastY = 1e9;
      [1, 4, 12].forEach((st) =>
        [-1, 1].forEach((sg) => {
          const y = G.cy - sg * G.yOf(st);
          g3.beginPath();
          g3.moveTo(G.cx - 3, y);
          g3.lineTo(G.cx + 3, y);
          g3.stroke();
          if (sg > 0 && lastY - y >= 11) {
            const lab = st === 12 ? tr("+12 半音", "+12 st") : `+${st}`;
            g3.fillText(lab, G.cx - (st === 12 ? g3.measureText(lab).width + 6 : 22), y + 3);
            lastY = y;
          }
        }),
      );
      // 点 1 つ = 粒 1 つ。位置・音程のずれは範囲の中で均等に散る(目盛りが平方根なので、絵では中心が粗く見える)
      const n = clamp(Math.round(d.density * 1.5), 6, 150);
      for (let i = 0; i < n; i++) {
        const u = rnd() * 2 - 1,
          v = rnd() * 2 - 1;
        const px = G.cx + Math.sign(u) * G.xOf(Math.abs(u) * d.spray),
          py = G.cy - Math.sign(v) * G.yOf(Math.abs(v) * d.pitchRand);
        g3.fillStyle = "rgba(232,196,106,0.85)";
        g3.beginPath();
        g3.arc(px, py, 2.5, 0, Math.PI * 2);
        g3.fill();
      }
      if (toolOf("cloud") === "points") {
        g3.beginPath();
        g3.arc(G.cx + rx, G.cy - ry, 6, 0, Math.PI * 2);
        g3.fillStyle = css("--human");
        g3.fill();
      }
      // 縦は音程(空間の上下ではない)。「低音」は左下(右下の ms の目盛りと重ねない)
      g3.fillStyle = css("--faint");
      g3.font = "10px sans-serif";
      const back = tr("後 →", "later →");
      g3.fillText(tr("← 前", "← earlier"), 4, G.cy - 4);
      g3.fillText(back, w3 - g3.measureText(back).width - 4, G.cy - 4);
      g3.fillText(tr("高音", "higher"), G.cx + 8, 12);
      const low = tr("低音", "lower");
      g3.fillText(low, G.cx - 8 - g3.measureText(low).width, h3 - 4);
      if (GRAN.hoverCloud) {
        const [hx, hy] = GRAN.hoverCloud;
        g3.strokeStyle = "rgba(255,255,255,0.2)";
        g3.beginPath();
        g3.moveTo(hx, 0);
        g3.lineTo(hx, h3);
        g3.moveTo(0, hy);
        g3.lineTo(w3, hy);
        g3.stroke();
      }
      const ci = byId("grCloudInfo");
      if (ci) {
        if (GRAN.hoverCloud) {
          const [hx, hy] = GRAN.hoverCloud,
            ux = clamp((hx - G.cx) / G.rx, -1, 1),
            uy = clamp((G.cy - hy) / G.ry, -1, 1);
          const ms = Math.round(Math.sign(ux) * 2000 * ux * ux),
            st = Math.round(Math.sign(uy) * 12 * uy * uy * 10) / 10;
          const inside = Math.abs(ms) <= d.spray && Math.abs(st) <= d.pitchRand;
          ci.textContent = tr(
            `位置のずれ ${ms >= 0 ? "+" : ""}${ms} ms · 音程のずれ ${st >= 0 ? "+" : ""}${st} 半音${inside ? "(粒が出る範囲)" : "(範囲の外)"}`,
            `Position offset ${ms >= 0 ? "+" : ""}${ms} ms · pitch offset ${st >= 0 ? "+" : ""}${st} st${inside ? " (inside the range)" : " (outside)"}`,
          );
        } else ci.textContent = tr(`位置のばらつき ±${d.spray} ms · 音程のばらつき ±${d.pitchRand} 半音`, `Spray ±${d.spray} ms · pitch randomness ±${d.pitchRand} st`);
      }
    }
    // 左右の帯: 粒を左右のどこに置くか(左右の広がり)。点 1 つ = 粒 1 つ
    const pc = byId<HTMLCanvasElement>("grPan");
    if (pc) {
      const { g: g5, w: w5, h: h5 } = ctx2d(pc),
        cx5 = w5 / 2,
        half = w5 / 2 - 24,
        my = h5 / 2;
      g5.strokeStyle = "#3a3a3a";
      g5.beginPath();
      g5.moveTo(cx5 - half, my);
      g5.lineTo(cx5 + half, my);
      g5.moveTo(cx5, 3);
      g5.lineTo(cx5, h5 - 3);
      g5.stroke();
      // 散る幅
      g5.fillStyle = "rgba(37,189,177,0.15)";
      g5.fillRect(cx5 - half * d.spread, 3, half * d.spread * 2, h5 - 6);
      const rp = seeded(11 + Math.round(d.density)),
        n5 = clamp(Math.round(d.density * 1.5), 6, 150);
      g5.fillStyle = "rgba(232,196,106,0.85)";
      for (let i = 0; i < n5; i++) {
        const xx = cx5 + (rp() * 2 - 1) * d.spread * half,
          y = my + (rp() * 2 - 1) * (h5 / 2 - 5);
        g5.beginPath();
        g5.arc(xx, y, 2, 0, Math.PI * 2);
        g5.fill();
      }
      // パン(ステレオの左右)
      g5.fillStyle = css("--faint");
      g5.font = "10px sans-serif";
      g5.fillText("L", 5, my + 4);
      g5.fillText("R", w5 - 11, my + 4);
      // つまむ所
      if (toolOf("cloud") === "points")
        [-1, 1].forEach((sg) => {
          g5.beginPath();
          g5.arc(cx5 + sg * half * d.spread, my, 5, 0, Math.PI * 2);
          g5.fillStyle = css("--human");
          g5.fill();
        });
    }
    // 散らばりの下の行: 帯にマウスを置いているときは左右の広がり
    const ci2 = byId("grCloudInfo");
    if (GRAN.hoverPan != null && ci2) ci2.textContent = tr(`左右の広がり ${d.spread.toFixed(2)}(マウスの所 ${GRAN.hoverPan.toFixed(2)})`, `Spread ${d.spread.toFixed(2)} (at the mouse ${GRAN.hoverPan.toFixed(2)})`);
    ui.panes.forEach((p) => p.draw());
  },
  /** 鍵盤を押している間、エンジンと同じ数え方で鳴っている粒を描く(1 / 粒の数 秒ごと、位置は進む速さで進み、ばらつきの中に散る) */
  held: 0,
  anim: 0,
  scanPos: 0,
  onKey(_m: number, on: boolean) {
    GRAN.held = Math.max(0, GRAN.held + (on ? 1 : -1));
    if (on && GRAN.held === 1) {
      const d0 = cur();
      GRAN.scanPos = d0.position;
      let next = performance.now();
      const tick = () => {
        if (!GRAN.held || state.inst !== "granular") {
          GRAN.anim = 0;
          GRAN.draw();
          return;
        }
        const dd = cur(),
          len = GRAN.len(dd),
          now = performance.now();
        while (next <= now) {
          GRAN.scanPos = (((GRAN.scanPos + dd.scan / dd.density / len) % 1) + 1) % 1;
          const off = clamp(GRAN.scanPos * len + (Math.random() * 2 - 1) * (dd.spray / 1000), 0, len);
          GRAN.grains.push({ t0: now, x: off / len, y: (Math.random() - 0.5) * 20 });
          next += 1000 / dd.density;
        }
        GRAN.draw();
        GRAN.anim = requestAnimationFrame(tick);
      };
      GRAN.anim = requestAnimationFrame(tick);
    }
  },
  refresh() {
    rerender();
  },
  help: (): [string, string][] => [
    [tr("素材", "Source"), tr("左で選ぶ。曲の中の音声クリップも使える。「ファイルから選ぶ…」で読み込める", "Pick on the left. Audio clips in the song work too. Load a file with “Choose a file…”")],
    [tr("取り出す位置を動かす", "Move the position"), tr("白い線(取り出す位置)をつまむか、クリックでそこへ", "Drag the white line (position) or click to jump there")],
    [tr("ばらつきを広げる", "Widen the spray"), tr("網掛けの端(青い線)をつまんで、取り出す範囲を広げる", "Drag the hatched area's edge (blue line) to widen where grains come from")],
    [
      tr("進み方を決める", "Set the scan"),
      tr("白い線から左右へドラッグ。押さえている間に取り出す位置が進む(粒そのものは動かない)。矢印の先 = 押さえて 1 秒後の位置(長いほど速い)", "Drag left/right from the white line. While a key is held the position moves (grains don't). Arrow tip = position after 1 s (longer = faster)"),
    ],
    [
      tr("粒 1 つの音量の形", "One grain's shape"),
      tr("粒 1 つに掛ける音量の付け方(なめらか・三角・台形・鋭い)と粒の長さ。灰 = 取り出した切れ端、水色 = 形を掛けた実際の粒。「長さをつまむ」で右端の ○ を動かす", "The level shape applied to each grain (smooth, triangle, trapezoid, sharp) and its length. Gray = the cut slice, teal = the actual grain. Use “Drag the length” to move the ○"),
    ],
    [tr("粒の散らばり", "Grain scatter"), tr("点 1 つ = 粒 1 つ。横 = 取り出す位置のずれ、縦 = 音程のずれ。下の帯 = 左右のどこに置くか(左右の広がり)。「角をつまむ」で ○ を動かす", "1 dot = 1 grain. Across = position offset, up/down = pitch offset. Bottom strip = left/right placement (spread). Use “Drag the corner” to move the ○")],
    [tr("全体の音量の変わり方", "Overall level over time"), tr("鍵盤を押してから離すまでの、音全体の音量(粒 1 つの形とは別)", "The whole sound's level from press to release (separate from one grain's shape)")],
  ],
};

register(GRAN);
export default GRAN;
