// 音色エディタ: サンプラー。素材 1 つを鍵盤で高さを変えて鳴らすか、区分に分けて鍵盤に並べる(スライス・チョップ)。
// 上 = 素材の波形(使う所・ループ・区分の線)と、区分 → 鍵盤をつなぐ帯、下 = 鍵盤での鳴らし方・フィルタ・音量の変わり方、右 = つまみ。
// 使う所(start / end)と手で決めた区分の線(slice_points)はエンジンのつまみ。自動の線は音の頭で探す(エンジンと同じ探し方)
import { open as pickFile } from "@tauri-apps/plugin-dialog";
import * as api from "../api";
import { tr } from "../i18n.svelte";
import { showToast } from "../toast.svelte";
import { draw as drawAll, holdVoice, playNotes, register, releaseVoice, setSel, toast, type Data, type Loaded, type Voice } from "./core.svelte";
import { byId, clamp, css, ctx2d, drawAudio, el, noteName, tag, timeRuler } from "./dom";
import {
  canEdit,
  cur,
  envMax,
  envPane,
  filterPane,
  knob,
  knobLabel,
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
  type LegendItem,
} from "./parts";

const r4 = (v: number) => Math.round(v * 1e4) / 1e4;
/** 素材の波形(最小・最大を交互に)と長さ。素材の ID ごと */
const waves: Record<string, { x: number[]; secs: number } | "loading"> = {};
/** 音の頭で自動に引く線(素材・使う所ごと) */
const autos = new Map<string, number[] | "loading">();

const SMP: any = {
  kind: "sampler",
  title: () => tr("サンプラー", "Sampler"),
  async load({ params, track }: Loaded): Promise<Data> {
    const n = (k: string, def: number) => (typeof params[k] === "number" ? params[k] : def);
    const b = (k: string, def: boolean) => (typeof params[k] === "boolean" ? params[k] : def);
    const src = (track.device as any)?.asset ?? "";
    const start = n("start", 0),
      end = n("end", 1);
    const sliceMode = n("slices", 0) > 0 ? "slice" : "off";
    let cuts = String(params.slice_points ?? "")
      .split(",")
      .map((t) => parseFloat(t))
      .filter((v) => Number.isFinite(v));
    // 線をまだ決めていない区分は、エンジンが音の頭で自動に引いた線(同じ探し方)
    if (sliceMode === "slice" && !cuts.length && src) cuts = await api.samplerAutoCuts(src, start, end).catch(() => []);
    return {
      src,
      root: Math.round(n("root", 60)),
      start,
      end,
      loop: b("loop", false),
      ls: n("loop_start", 0),
      le: n("loop_end", 1),
      xf: n("loop_xfade_ms", 10),
      sliceMode,
      cuts,
      a: n("attack_ms", 2) / 1000,
      d: n("decay_ms", 1000) / 1000,
      s: n("sustain", 1),
      r: n("release_ms", 80) / 1000,
      ftype: typeof params.filter_type === "string" ? params.filter_type : "off",
      cutoff: n("cutoff", 20000),
      res: n("resonance", 0),
      vcut: n("vel_cutoff", 0),
      fenv: n("filter_env", 0),
      keyTrack: b("key_track", true),
      stereo: b("stereo", false),
      gain: n("gain_db", 0),
      origBpm: n("orig_bpm", 0),
    };
  },
  uiInit: (): Data => ({ selSlice: null, selCut: null, showShelf: false }),
  params: (d: Data) => ({
    root: d.root,
    start: r4(d.start),
    end: r4(d.end),
    loop: d.loop,
    loop_start: r4(d.ls),
    loop_end: r4(d.le),
    loop_xfade_ms: d.xf,
    slices: d.sliceMode === "off" ? 0 : clamp(SMP.inUse(d, d.cuts).length + 1, 1, 64),
    slice_points: [...d.cuts].sort((a: number, b: number) => a - b).map(r4).join(","),
    attack_ms: r4(d.a * 1000),
    decay_ms: r4(d.d * 1000),
    sustain: d.s,
    release_ms: r4(d.r * 1000),
    filter_type: d.ftype,
    cutoff: d.cutoff,
    resonance: d.res,
    vel_cutoff: d.vcut,
    filter_env: d.fenv,
    key_track: d.keyTrack,
    stereo: d.stereo,
    gain_db: d.gain,
    orig_bpm: d.origBpm,
  }),
  /** 素材を替えたら音源の素材を差し替える(つまみはそのまま) */
  extraCommands(trackId: string, prev: Data, next: Data) {
    if (prev.src === next.src || !next.src) return [];
    const dv: any = state.project?.tracks.find((t) => t.id === trackId)?.device ?? {};
    return [{ op: "set_device", track: trackId, device: { type: "sampler", asset: next.src, params: { ...(dv.params ?? {}), ...SMP.params(next) } } }];
  },
  hoverX: null as number | null,
  linkHover: null as any,
  pianoHover: null as number | null,
  lit: null as number | null,
  cutAdded: false,
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
    // このトラックの素材は、このトラックの名前で
    for (const t of [...p.tracks].sort((a, b) => +(b.id === state.track) - +(a.id === state.track))) {
      const dv: any = t.device;
      const id = dv?.type === "sampler" ? dv.asset : dv?.name === "granular" ? dv.params?.sample : null;
      if (typeof id !== "string" || !id || out.has(id)) continue;
      const s = secsOf(id);
      out.set(id, { id, name: t.name, desc: tr(`「${t.name}」の素材 · ${s.toFixed(1)} 秒`, `Source of "${t.name}" · ${s.toFixed(1)} s`), secs: s });
    }
    return [...out.values()];
  },
  wave(id: string): number[] | null {
    if (!id) return null;
    const w = waves[id];
    if (w && w !== "loading") return w.x;
    if (!w) {
      waves[id] = "loading";
      api
        .assetPeaks(id, 2400)
        .then((r) => {
          waves[id] = { x: r.peaks, secs: r.seconds };
          if (state.inst === "sampler") rerender();
        })
        .catch(() => delete waves[id]);
    }
    return null;
  },
  len(d: Data) {
    const w = waves[d.src];
    if (w && w !== "loading") return Math.max(0.01, w.secs);
    const s = SMP.sources().find((x: any) => x.id === d.src);
    return Math.max(0.01, s?.secs || 1);
  },
  /** 区分の頭: 使う所の始まり + 使う所の中の線。1 つの音のときは空 */
  cuts(d: Data): number[] {
    return d.sliceMode === "off" ? [] : [d.start, ...SMP.inUse(d, d.cuts)];
  },
  /** 使う所(始まり〜終わり)の中にある線だけ(並べて)。外の線は消さずに残し、使わない(使う所を広げると戻る) */
  inUse: (d: Data, cs: number[]) => [...cs].filter((c) => c > d.start + 0.005 && c < d.end - 0.005).sort((a, b) => a - b),
  /** 音の頭で自動に引く線(使う所の中だけ)。エンジンで探す(届くまでは null) */
  autoCuts(d: Data): number[] | null {
    if (!d.src) return [];
    const key = JSON.stringify([d.src, r4(d.start), r4(d.end)]);
    const a = autos.get(key);
    if (a === "loading") return null;
    if (a) return a;
    autos.set(key, "loading");
    api
      .samplerAutoCuts(d.src, d.start, d.end)
      .then((r) => {
        autos.set(key, r);
        if (state.inst === "sampler") SMP.draw();
      })
      .catch(() => autos.delete(key));
    return null;
  },
  /** 選んだ切れ目を消す(右クリック・Delete キー) */
  removeCut(d: Data, i: number) {
    if (i == null || i < 0 || i >= d.cuts.length) return;
    d.cuts.splice(i, 1);
    d.selCut = null;
    d.selSlice = null;
    pushHist(tr("切れ目を消す", "Remove a cut"));
    rerender();
  },
  onDelete() {
    const d = cur();
    if (toolOf("wave") !== "cut" || d.selCut == null || !canEdit()) return false;
    SMP.removeCut(d, d.selCut);
    return true;
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
    const srcs = SMP.sources();
    if (!srcs.length) clips.append(el("div", { class: "hint" }, tr("曲の中に音声がありません。下の「ファイルから選ぶ…」で読み込めます", "No audio in the song yet. Load one with “Choose a file…” below")));
    srcs.forEach((s: any) => {
      const c = el("canvas");
      clips.append(
        el(
          "div",
          {
            class: "clipitem" + (d.src === s.id ? " on" : ""),
            onclick: async () => {
              if (!canEdit() || d.src === s.id) return;
              const cuts = d.sliceMode === "off" ? d.cuts : await api.samplerAutoCuts(s.id, 0, 1).catch(() => []);
              Object.assign(d, { src: s.id, cuts, start: 0, end: 1, selSlice: null, selCut: null });
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
        const x = SMP.wave(s.id);
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
            title: tr("鳴らす音声を選ぶ", "Choose audio to play"),
            filters: [{ name: tr("音声(WAV / MP3 / FLAC / OGG / M4A)", "Audio (WAV / MP3 / FLAC / OGG / M4A)"), extensions: ["wav", "mp3", "flac", "ogg", "m4a", "aac"] }],
          });
          if (typeof file !== "string") return;
          try {
            await api.importSample(state.track, file);
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
        el("h3", {}, tr("素材", "Source"), el("span", { class: "spacer" }), el("span", { class: "hint" }, tr("鳴らす音声", "Audio to play"))),
        srcSeg,
        d.showShelf ? shelfList("sampler") : clips,
        d.showShelf ? null : pickBtn,
      ),
    );
    // ---- 真ん中 ----
    // 上は波形と鍵盤へのつながりを縦に並べるので高め。低い画面では真ん中の列ごとスクロール
    const center = el("div", { class: "col center wtcol", style: "overflow-y: auto" });
    const grid = el("div", { class: "wtgrid", style: "grid-template-rows: minmax(min-content, 1fr) minmax(290px, 1.08fr)" });
    // 区分のときループの道具は使えない
    if (d.sliceMode !== "off" && toolOf("wave") === "loop") ui.tool[`${state.track}:wave`] = "view";
    const tool = toolOf("wave");
    const wc = el("canvas", { id: "smpWave", class: "fill", style: "min-height: 96px" });
    // 波形の真下: 区分(か素材全体)から、それを鳴らす鍵盤へ帯でつなぐ絵。押している間鳴らす
    const link = el("canvas", { id: "smpLink", style: "height: 48px; flex: none; cursor: pointer" });
    const winfo = el("div", { class: "paneinfo", id: "smpInfo" });
    // 鍵盤での鳴らし方: 1 つの音として弾くか、区分に分けて並べるか。区分の線はいつでも動かせる(「音の頭で切り直す」で自動で引き直す)
    const setMode = async (k: string) => {
      if (!canEdit() || d.sliceMode === k) return;
      const auto = k === "slice" && !d.cuts.length ? await api.samplerAutoCuts(d.src, d.start, d.end).catch(() => []) : null;
      if (k !== "off" && toolOf("wave") === "loop") ui.tool[`${state.track}:wave`] = "view";
      if (k === "off" && toolOf("wave") === "cut") ui.tool[`${state.track}:wave`] = "view";
      d.sliceMode = k;
      d.selSlice = null;
      d.selCut = null;
      if (auto) d.cuts = auto;
      pushHist(k === "off" ? tr("1 つの音として高さを変えて弾く", "Play as one sound, pitched by key") : tr("区分に分けて鍵盤に並べる", "Split into slices across the keys"));
      rerender();
    };
    const modeSeg = el("div", { class: "seg", style: "display: grid; grid-template-columns: 1fr" });
    (
      [
        ["off", tr("1 つの音として高さを変えて弾く", "Play as one sound, pitched by key"), tr("素材全体を 1 つの音にして、どの鍵盤でも同じ素材を、鍵盤の高さに合わせて速さを変えて鳴らす", "Treat the whole source as one sound; every key plays it, sped up or slowed down to match the key")],
        ["slice", tr("区分に分けて鍵盤に並べる", "Split into slices across the keys"), tr("素材を区分に分けて、区分 1 つを鍵盤 1 つに並べる(スライス・チョップ)。どの区分も録音どおりの速さで鳴る", "Split the source into slices, one per key (slicing / chopping). Each slice plays at the recording's own speed")],
      ] as [string, string, string][]
    ).forEach(([k, n, tip]) => modeSeg.append(el("button", { class: (k === "off") === (d.sliceMode === "off") ? "on" : "", title: tip, style: "text-align: left; border-left: 0", onclick: () => setMode(k) }, n)));
    const recut = el(
      "button",
      {
        class: "btn",
        title: tr(
          "使う所の中で、音が急に大きくなる所(ドラムの 1 打・声の 1 音節)を探して、区分の線を引き直す。手で動かした線・使う所の外の線は消える",
          "Find where the sound suddenly gets louder (a drum hit, a syllable) inside the used part and redraw the slice lines. Moved lines and lines outside the used part are removed",
        ),
        onclick: async () => {
          if (!canEdit()) return;
          d.cuts = await api.samplerAutoCuts(d.src, d.start, d.end).catch(() => d.cuts);
          d.selSlice = null;
          d.selCut = null;
          pushHist(tr("音の頭で区分を切り直す", "Re-slice at the onsets"));
          rerender();
        },
      },
      tr("音の頭で切り直す", "Re-slice at onsets"),
    );
    const piano = el("canvas", { id: "smpPiano", class: "fill", style: "cursor: pointer" });
    const pinfo = el("span", { class: "hint", id: "smpPianoInfo" });
    const rootName = SMP.rootName(d);
    const sliced = d.sliceMode !== "off",
      NO_LOOP = tr(
        "区分に分けて並べるときはループしない(各区分は次の区分の頭まで鳴って止まる)。ループは「1 つの音として高さを変えて弾く」のときだけ",
        "Slices don't loop (each plays until the next slice's start). Looping works only with “Play as one sound”",
      );
    const loopBox = () =>
      el(
        "label",
        { class: "small" + (sliced ? " dim" : ""), title: sliced ? NO_LOOP : tr("押さえている間、水色の ▲ の間を繰り返して音を伸ばす", "While a key is held, repeat between the teal ▲ marks to sustain") },
        el("input", {
          type: "checkbox",
          checked: d.loop && !sliced,
          disabled: sliced,
          onchange: (e: Event) => {
            if (!canEdit()) return;
            d.loop = (e.target as HTMLInputElement).checked;
            pushHist(d.loop ? tr("ループを入れる", "Turn on the loop") : tr("ループを切る", "Turn off the loop"));
            rerender();
          },
        }),
        tr(" 押さえている間ループで伸ばす", " Loop while held"),
      );
    const xfKnob = () =>
      SMP.withXfExplain(
        knob("つなぎ目", {
          min: 0,
          max: 1000,
          step: 1,
          value: d.xf,
          unit: " ms",
          onInput: (v) => {
            d.xf = v;
            drawAll();
          },
          onCommit: (v) => pushHist(tr(`ループのつなぎ目を ${v} ms に`, `Loop crossfade → ${v} ms`)),
        }),
      );
    const srcName = srcs.find((s: any) => s.id === d.src)?.name ?? (d.src ? d.src.slice(0, 18) : tr("(なし)", "(none)"));
    const legend: LegendItem[] = [
      ["unused", tr("使わない所", "Unused")],
      ...(sliced
        ? []
        : ([
            ["loopa", tr("ループ", "Loop")],
            ["xf", tr("つなぎ目", "Crossfade")],
          ] as LegendItem[])),
      ...(sliced ? ([["cutl", tr("切れ目", "Cut")]] as LegendItem[]) : []),
      ...(sliced && d.cuts.length > SMP.inUse(d, d.cuts).length
        ? ([
            [
              el("span", { class: "sw", style: "width: 3px; background: repeating-linear-gradient(180deg, #a0a0a0 0 3px, transparent 3px 6px)" }),
              tr("使わない切れ目", "Unused cut"),
              tr("使う所の外の線。消さずに残すので、使う所を広げるとまた区分になる", "A line outside the used part. It's kept, so widening the used part makes it a slice again"),
            ],
          ] as LegendItem[])
        : []),
      ...(sliced
        ? ([
            [el("span", { class: "sw", style: "background: linear-gradient(90deg, #e8c46a, #e88a6a, #d46ab8, #9a7ae8)" }), tr("色 = 鳴らす鍵盤", "Color = key"), tr("区分の区間・帯・鍵盤は同じ色", "A slice's region, band and key share a color")],
            ["now", tr("選んだ区分", "Selected slice")],
          ] as LegendItem[])
        : []),
      ["lit", tr("鳴っている", "Sounding")],
    ];
    grid.append(
      el(
        "div",
        { class: "box", style: "grid-column: span 4" },
        el("h3", {}, tr(`素材の波形: ${srcName}`, `Source waveform: ${srcName}`)),
        toolSeg(
          [
            [VIEW_TOOL()[0], VIEW_TOOL()[1], tr("見るだけ。区分をクリックすると選んで鳴らす", "View only. Click a slice to select and play it")],
            ["trim", tr("使う所を切る", "Trim"), tr("白い ▼ をつまんで、始まりと終わりを決める", "Drag the white ▼ marks to set the start and end"), "trim"],
            ["loop", tr("ループを決める", "Set the loop"), tr("水色の ▲ をつまんで、ループする所を決める", "Drag the teal ▲ marks to set the loop"), "loop", sliced ? NO_LOOP : undefined],
            [
              "cut",
              tr("切れ目を調整", "Edit cuts"),
              tr("黄色の線をつまんで動かす。線の無い所をクリックで足す。線をクリックして Delete キー、または線を右クリックで消す", "Drag the yellow lines. Click an empty spot to add one. Select a line and press Delete, or right-click it, to remove it"),
              "cut",
            ],
          ],
          tool,
          async (t) => {
            if (t === "cut" && d.sliceMode === "off") {
              if (!canEdit()) return;
              const auto = d.cuts.length ? null : await api.samplerAutoCuts(d.src, d.start, d.end).catch(() => []);
              d.sliceMode = "slice";
              if (auto) d.cuts = auto;
              pushHist(tr("区分に分けて鍵盤に並べる", "Split into slices across the keys"));
            }
            d.selCut = null;
            setTool("wave", t);
          },
        ),
        wc,
        link,
        winfo,
        legendRow(legend),
        // 「ループを決める」のときだけ: ループの入り切りとつなぎ目
        tool === "loop" && !sliced ? el("div", { class: "toolrow" }, loopBox(), d.loop ? xfKnob() : null) : null,
      ),
    );
    grid.append(specBox("grid-column: span 2"));
    // 下: 鳴らし方
    grid.append(
      el(
        "div",
        { class: "box editgroup", style: "grid-column: span 6" },
        el("div", { class: "edithead" }, el("b", {}, tr("鳴らし方の調整", "How it plays")), el("span", { class: "spacer" }), el("span", { class: "hint" }, d.sliceMode === "off" ? tr("素材 1 つを、鍵盤で高さを変えて鳴らす", "One source, pitched by key") : tr("区分を 1 つずつ鍵盤に並べて鳴らす", "One slice per key"))),
        el(
          "div",
          { class: "editpanes three" },
          pane(tr("鍵盤での鳴らし方", "How keys play it"), {
            info: pinfo,
            tools: el(
              "div",
              { class: "howpane" },
              modeSeg,
              d.sliceMode === "off" ? null : el("div", { class: "toolrow" }, recut),
              knob(rootName, {
                cls: "wide",
                bind: `${state.track}:root`,
                min: 0,
                max: 127,
                step: 1,
                value: d.root,
                fmt: (v) => noteName(v),
                title:
                  d.sliceMode === "off"
                    ? tr("この鍵盤で弾くと録音どおりの速さ(高さ)で鳴る。ほかの鍵盤は半音の差の分だけ速さを変える(元の名前: ルート音程)", "Playing this key plays the recording at its own speed (pitch). Other keys change the speed by the semitone difference (root note)")
                    : tr("区分 1 の鍵盤。区分 2 からは半音ずつ上へ並ぶ(元の名前: ルート音程)", "The key of slice 1. Slice 2 onward go up a semitone each (root note)"),
                onInput: (v) => {
                  d.root = v;
                  drawAll();
                },
                onCommit: (v) => pushHist(tr(`${rootName}を ${noteName(v)} に`, `${knobLabel(rootName)} → ${noteName(v)}`)),
              }),
            ),
            canvas: piano,
          }),
          filterPane("smpFilt", {
            get: () => {
              const D = shown();
              return { type: D.ftype, cutoff: D.cutoff, res: D.res };
            },
            set: (k, v) => {
              const D = cur();
              if (k === "type") D.ftype = v;
              else D[k] = v;
            },
          }),
          envPane("smpEnv", { get: () => shown() as any, set: (k, v) => (cur()[k] = v), max: { a: 5, d: 10, r: 2 } }),
        ),
      ),
    );
    center.append(grid);
    // ---- 右: つまみ ----
    const right = el("div", { class: "col right" });
    const k = (label: string, key: string, min: number, max: number, step: number, unit = "", opts: any = {}) => knobOf(d, label, key, min, max, step, unit, opts);
    const check = (label: string, key: string, on: string, off: string) =>
      el(
        "label",
        { class: "small dim" },
        el("input", {
          type: "checkbox",
          checked: d[key],
          onchange: (e: Event) => {
            if (!canEdit()) return;
            d[key] = (e.target as HTMLInputElement).checked;
            pushHist(d[key] ? on : off);
            drawAll();
          },
        }),
        label,
      );
    right.append(
      el(
        "div",
        { class: "box" },
        el("h3", {}, tr("鳴らし方", "Playing")),
        knob(rootName, {
          min: 0,
          max: 127,
          step: 1,
          value: d.root,
          fmt: (v) => noteName(v),
          title: d.sliceMode === "off" ? tr("この鍵盤で弾くと録音どおりの速さ(高さ)で鳴る(元の名前: ルート音程)", "Playing this key plays the recording at its own speed (root note)") : tr("区分 1 を置く鍵盤(元の名前: ルート音程)", "The key for slice 1 (root note)"),
          bind: `${state.track}:root`,
          onInput: (v) => {
            d.root = v;
            drawAll();
          },
          onCommit: (v) => pushHist(tr(`${rootName}を ${noteName(v)} に`, `${knobLabel(rootName)} → ${noteName(v)}`)),
        }),
        check(tr(" 鍵盤で高さを変える(キー追従)", " Pitch follows the key (key tracking)"), "keyTrack", tr("キー追従を入れる", "Turn on key tracking"), tr("キー追従を切る", "Turn off key tracking")),
        loopBox(),
        d.loop && !sliced ? xfKnob() : null,
        check(tr(" ステレオのまま鳴らす", " Keep it stereo"), "stereo", tr("ステレオを入れる", "Turn on stereo"), tr("ステレオを切る", "Turn off stereo")),
        k("元のテンポ", "origBpm", 0, 300, 1, " BPM", { title: tr("0 = 合わせない。入れると曲のテンポに合わせて伸び縮み", "0 = don't follow. Set it to stretch with the song's tempo") }),
        k("ゲイン", "gain", -24, 12, 0.5, " dB"),
      ),
    );
    right.append(
      el(
        "div",
        { class: "box" },
        el("h3", {}, tr("フィルタ", "Filter")),
        k("カットオフ", "cutoff", 20, 20000, 10, " Hz"),
        k("レゾナンス", "res", 0, 0.95, 0.01),
        k("強さで開く", "vcut", 0, 1, 0.01, "", { title: tr("強く弾くほどカットオフが上がる", "Harder playing raises the cutoff") }),
        k("時間で開く", "fenv", 0, 1, 0.01, "", { title: tr("フィルタの変わり方: 音量の変わり方に合わせて、カットオフを動かす量", "How much the cutoff follows the level envelope") }),
      ),
    );
    right.append(
      el(
        "div",
        { class: "box" },
        el("h3", {}, tr("音量の変わり方", "Level over time"), el("span", { class: "spacer" }), el("span", { class: "hint" }, tr("押す → 離す", "press → release"))),
        k("立ち上がり", "a", 0.0005, envMax("smpEnv", 5), 0.0005, " s"),
        k("下がる時間", "d", 0.005, envMax("smpEnv", 10), 0.005, " s"),
        k("保つ音量", "s", 0, 1, 0.01),
        k("余韻", "r", 0.005, envMax("smpEnv", 2), 0.005, " s"),
      ),
    );
    right.append(saveBox(state.trackName, tr("素材・切れ目・ループ", "the source, cuts and loop")));
    body.append(left, center, right);
    // ---- 波形の操作 ----
    let drag: string | null = null;
    const fxOf = (ev: MouseEvent) => {
      const r = wc.getBoundingClientRect();
      return clamp((ev.clientX - r.left) / r.width, 0, 1);
    };
    const cutNear = (fx: number) => d.cuts.findIndex((c: number) => Math.abs(c - fx) * wc.getBoundingClientRect().width < 8);
    const near = (fx: number, keys: [string, number][]) => {
      const r = wc.getBoundingClientRect();
      const c = keys.map(([k, v]) => [k, Math.abs(v - fx) * r.width] as [string, number]).sort((a, b) => a[1] - b[1])[0];
      return c && c[1] < 10 ? c[0] : null;
    };
    wc.addEventListener("pointerdown", (ev) => {
      const fx = fxOf(ev);
      if (tool === "view") {
        const cuts = SMP.cuts(d);
        if (!cuts.length) return;
        // 使う所の外(区分は無い)
        if (fx < d.start || fx > d.end) return;
        const si = cuts.filter((c: number) => c <= fx).length - 1;
        d.selSlice = si;
        setSel(tr(`区分 ${si + 1}`, `Slice ${si + 1}`));
        SMP.draw();
        SMP.audition(si);
        return;
      }
      if (!canEdit()) return;
      if (tool === "trim")
        drag = near(fx, [
          ["start", d.start],
          ["end", d.end],
        ]);
      else if (tool === "loop")
        drag =
          d.loop && d.sliceMode === "off"
            ? near(fx, [
                ["ls", d.ls],
                ["le", d.le],
              ])
            : null;
      else if (tool === "cut") {
        // 右クリックは消す(contextmenu)
        if (ev.button !== 0) return;
        const i = cutNear(fx);
        if (i >= 0) drag = "cut" + i;
        else if (fx > d.start && fx < d.end && SMP.inUse(d, d.cuts).length >= 63) {
          toast(tr("区分は 64 までです(エンジンの上限)", "Up to 64 slices (the engine's limit)"));
          return;
        } else if (fx > d.start && fx < d.end) {
          d.cuts.push(fx);
          drag = "cut" + (d.cuts.length - 1);
          SMP.cutAdded = true;
        } else {
          toast(tr("使う所の外には線を足せません(使う所は「使う所を切る」で広げる)", "You can't add a line outside the used part (widen it with “Trim”)"));
          return;
        }
        d.selCut = +drag.slice(3);
        SMP.draw();
      }
      if (drag) wc.setPointerCapture(ev.pointerId);
    });
    // 切れ目を右クリックで消す
    wc.addEventListener("contextmenu", (ev) => {
      if (tool !== "cut") return;
      ev.preventDefault();
      if (!canEdit()) return;
      SMP.removeCut(d, cutNear(fxOf(ev)));
    });
    wc.addEventListener("pointermove", (ev) => {
      const fx = fxOf(ev);
      SMP.hoverX = fx;
      if (!drag) {
        const can =
          tool === "trim"
            ? near(fx, [
                ["start", d.start],
                ["end", d.end],
              ])
            : tool === "loop"
              ? near(fx, [
                  ["ls", d.ls],
                  ["le", d.le],
                ])
              : tool === "cut"
                ? cutNear(fx) >= 0
                  ? "c"
                  : null
                : null;
        wc.style.cursor = can ? "ew-resize" : tool === "cut" ? "copy" : tool === "view" ? "pointer" : "default";
        SMP.draw();
        return;
      }
      if (drag.startsWith("cut")) d.cuts[+drag.slice(3)] = fx;
      else if (drag === "start") d.start = Math.min(fx, d.end - 0.02);
      else if (drag === "end") d.end = Math.max(fx, d.start + 0.02);
      else if (drag === "ls") d.ls = Math.min(fx, d.le - 0.02);
      else d.le = Math.max(fx, d.ls + 0.02);
      drawAll();
    });
    wc.addEventListener("pointerup", () => {
      if (!drag) return;
      if (drag.startsWith("cut")) {
        const v = d.cuts[d.selCut];
        d.cuts.sort((a: number, b: number) => a - b);
        d.selCut = d.cuts.indexOf(v);
      }
      if (drag === "start" || drag === "end" || drag.startsWith("cut")) d.selSlice = null;
      pushHist(
        ({
          start: tr("始まりを動かす", "Move the start"),
          end: tr("終わりを動かす", "Move the end"),
          ls: tr("ループの始まりを動かす", "Move the loop start"),
          le: tr("ループの終わりを動かす", "Move the loop end"),
        } as Record<string, string>)[drag] ?? (SMP.cutAdded ? tr("切れ目を足す", "Add a cut") : tr("切れ目を動かす", "Move a cut")),
      );
      SMP.cutAdded = false;
      drag = null;
      rerender();
    });
    wc.addEventListener("pointerleave", () => {
      SMP.hoverX = null;
      SMP.draw();
    });
    // ---- 鍵盤へのつながり: 押している間鳴らす(区分のある鍵盤はその区分を選ぶ) ----
    const linkAt = (ev: PointerEvent) => {
      const r = link.getBoundingClientRect();
      return SMP.linkHit(cur(), ev.clientX - r.left, ev.clientY - r.top, r.width, r.height);
    };
    let lv: Voice | null = null;
    link.addEventListener("pointerdown", (ev) => {
      const L = linkAt(ev);
      if (!L || L.none) return;
      const D = cur();
      if (L.si != null) {
        D.selSlice = L.si;
        setSel(tr(`区分 ${L.si + 1}`, `Slice ${L.si + 1}`));
      }
      lv = holdVoice(playNotes([L.m]));
      SMP.lit = L.m;
      link.setPointerCapture(ev.pointerId);
      SMP.draw();
    });
    const linkUp = () => {
      if (!lv) return;
      releaseVoice(lv);
      lv = null;
      SMP.lit = null;
      SMP.draw();
    };
    link.addEventListener("pointerup", linkUp);
    link.addEventListener("pointercancel", linkUp);
    link.addEventListener("pointermove", (ev) => {
      SMP.linkHover = linkAt(ev);
      SMP.draw();
    });
    link.addEventListener("pointerleave", () => {
      SMP.linkHover = null;
      SMP.draw();
    });
    // ---- 下の鍵盤: 押している間鳴らす(区分のある鍵盤はその区分を選ぶ) ----
    const pianoAt = (ev: PointerEvent) => {
      const r = piano.getBoundingClientRect();
      return SMP.pianoHit(cur(), ev.clientX - r.left, ev.clientY - r.top, r.width, r.height);
    };
    let pv: Voice | null = null;
    piano.addEventListener("pointerdown", (ev) => {
      const m = pianoAt(ev);
      if (m == null) return;
      const D = cur(),
        si = D.sliceMode === "off" ? null : m - D.root;
      if (si != null && (si < 0 || si >= SMP.cuts(D).length)) return;
      if (si != null) {
        D.selSlice = si;
        setSel(tr(`区分 ${si + 1}`, `Slice ${si + 1}`));
      }
      pv = holdVoice(playNotes([m]));
      SMP.lit = m;
      piano.setPointerCapture(ev.pointerId);
      SMP.draw();
    });
    const pianoUp = () => {
      if (pv) releaseVoice(pv);
      pv = null;
      SMP.lit = null;
      SMP.draw();
    };
    piano.addEventListener("pointerup", pianoUp);
    piano.addEventListener("pointercancel", pianoUp);
    piano.addEventListener("pointermove", (ev) => {
      SMP.pianoHover = pianoAt(ev);
      SMP.draw();
    });
    piano.addEventListener("pointerleave", () => {
      SMP.pianoHover = null;
      SMP.draw();
    });
  },
  /** 区分の線を手で動かしたか(音の頭で自動に引いた線のままでないか) */
  handCut(d: Data) {
    const a = SMP.autoCuts(d);
    if (!a) return false;
    const mine = SMP.inUse(d, d.cuts).map(r4);
    return JSON.stringify(mine) !== JSON.stringify(a.map(r4));
  },
  /** 下の鍵盤の並び(本物の鍵盤の形: 白鍵の上に黒鍵)。1 つの音のときは録音どおりの鍵盤の上下 1 オクターブ、区分のときは区分 1 の鍵盤のオクターブから */
  pianoRange(d: Data): [number, number] {
    const c = d.root - (((d.root % 12) + 12) % 12);
    // MIDI の鍵盤(0〜127)の中だけ
    const fit = ([a, b]: [number, number]): [number, number] => [Math.max(0, a), Math.min(127, b)];
    if (d.sliceMode === "off") return fit([c - 12, c + 12]);
    return fit([c, Math.max(c + 24, c + Math.ceil((d.root - c + SMP.cuts(d).length + 1) / 12) * 12)]);
  },
  pianoKeys(d: Data, w: number, h: number) {
    const [lo, hi] = SMP.pianoRange(d),
      isB = (m: number) => [1, 3, 6, 8, 10].includes(((m % 12) + 12) % 12);
    const whites: number[] = [];
    for (let m = lo; m <= hi; m++) if (!isB(m)) whites.push(m);
    const ww = w / whites.length,
      keys: { m: number; black: boolean; x: number; w: number; y: number; h: number }[] = [];
    whites.forEach((m, i) => keys.push({ m, black: false, x: i * ww, w: ww, y: 0, h }));
    for (let m = lo; m <= hi; m++)
      if (isB(m)) {
        const i = whites.indexOf(m - 1);
        keys.push({ m, black: true, x: (i + 1) * ww - ww * 0.32, w: ww * 0.64, y: 0, h: h * 0.6 });
      }
    return keys;
  },
  pianoHit(d: Data, x: number, y: number, w: number, h: number) {
    const K = SMP.pianoKeys(d, w, h);
    const b = K.filter((k: any) => k.black).find((k: any) => x >= k.x && x <= k.x + k.w && y <= k.h);
    if (b) return b.m;
    const k = K.find((k: any) => !k.black && x >= k.x && x < k.x + k.w);
    return k ? k.m : null;
  },
  /** 「つなぎ目」の説明: マウスを置くと、つなぎ目 0 と あり の違いを小さな絵で見せる */
  withXfExplain(node: HTMLElement) {
    const host = () => (document.querySelector(".sound-editor") ?? document.body) as HTMLElement;
    const pop = () => (host().querySelector(".explain.xf") as HTMLElement | null) ?? host().appendChild(el("div", { class: "explain xf" }));
    node.addEventListener("mouseenter", () => {
      const p = pop();
      p.innerHTML = "";
      const c0 = el("canvas", { style: "width: 100%; height: 54px" }),
        c1 = el("canvas", { style: "width: 100%; height: 54px" });
      p.append(
        el("b", {}, tr("つなぎ目(ループのクロスフェード)", "Crossfade (loop crossfade)")),
        el(
          "div",
          { class: "small" },
          tr(
            "ループは、ループの終わりまで来たら始まりへ戻って繰り返します。終わりと始まりの波の形はたいてい合わないので、そのまま戻ると波が飛んで「プツッ」と鳴ります。",
            "A loop jumps back to its start when it reaches its end. The wave at the end and the start rarely match, so jumping straight back makes the wave skip and click.",
          ),
        ),
        c0,
        el("div", { class: "small" }, tr("つなぎ目 0 ms: 戻る所で波が飛ぶ(プツッ)", "0 ms crossfade: the wave skips at the jump (click)")),
        c1,
        el(
          "div",
          { class: "small" },
          tr(
            "つなぎ目あり: 終わりを小さくしながら始まりを大きくして、その長さだけ重ねる(なめらかにつながる)。長いほどなめらかだが、長すぎると重なった所の音がにじむ",
            "With a crossfade: the end fades out while the start fades in, overlapping for that long (a smooth join). Longer is smoother, but too long smears the overlap",
          ),
        ),
      );
      p.classList.add("open");
      const r = node.getBoundingClientRect(),
        ph = p.offsetHeight;
      p.style.left = `${clamp(r.left, 8, window.innerWidth - p.offsetWidth - 8)}px`;
      p.style.top = `${r.bottom + 6 + ph < window.innerHeight ? r.bottom + 6 : Math.max(8, r.top - ph - 6)}px`;
      [c0, c1].forEach((c, k) => {
        const { g, w, h } = ctx2d(c),
          mid = w / 2,
          y0 = h / 2,
          A = h * 0.32;
        g.strokeStyle = "#333";
        g.beginPath();
        g.moveTo(mid, 0);
        g.lineTo(mid, h);
        g.stroke();
        g.fillStyle = css("--faint");
        g.font = "9px sans-serif";
        g.fillText(tr("ループの終わり", "Loop end"), 2, 9);
        g.fillText(tr("→ 始まりへ戻る", "→ back to start"), mid + 4, 9);
        g.strokeStyle = css("--accent");
        g.lineWidth = 1.5;
        g.beginPath();
        for (let x = 0; x <= w; x++) {
          // 左は終わりの波(戻る所で山の頂上)、右は始まりの波(谷から始まる)。つなぎ目ありは真ん中の前後で混ぜる
          const ph2 = Math.PI / 2 - mid * 0.25,
            a = Math.sin(x * 0.25 + ph2) * A,
            b = Math.sin(x * 0.25 + ph2 + Math.PI) * A * 0.9;
          const t = k === 0 ? (x < mid ? 0 : 1) : clamp((x - (mid - 26)) / 52, 0, 1);
          const v = a * (1 - t) + b * t;
          if (x === 0) g.moveTo(x, y0 - v);
          else g.lineTo(x, y0 - v);
        }
        g.stroke();
        if (k === 1) {
          g.fillStyle = "rgba(37,189,177,0.15)";
          g.fillRect(mid - 26, 0, 52, h);
          g.fillStyle = css("--dim");
          g.fillText(tr("重ねる長さ", "overlap"), mid - 22, h - 3);
        } else {
          g.strokeStyle = css("--human");
          g.lineWidth = 1;
          g.beginPath();
          g.moveTo(mid + 6, y0 - A);
          g.lineTo(mid + 6, y0 + A * 0.9);
          g.stroke();
          g.fillStyle = css("--human");
          g.fillText(tr("飛ぶ", "skip"), mid + 9, y0 + 3);
        }
      });
    });
    node.addEventListener("mouseleave", () => pop().classList.remove("open"));
    return node;
  },
  /** 「元の高さ」の名前(区分に分けているかで意味が変わるので言い分ける) */
  rootName: (d: Data) => (d.sliceMode === "off" ? "録音どおりの鍵盤" : "区分 1 の鍵盤"),
  /** 区分の色(波形の区間・帯・鍵盤で同じ色) */
  SLICE_COLORS: ["232,196,106", "232,138,106", "212,106,184", "154,122,232", "106,154,232", "138,212,106", "232,163,58", "200,212,106"],
  sliceColor: (i: number, a: number) => `rgba(${SMP.SLICE_COLORS[i % SMP.SLICE_COLORS.length]},${a})`,
  /** つながりの絵の鍵盤の並び: 区分のときは区分の数 + 前後、1 つの音のときは上下 1 オクターブ(0〜127 の中に収める) */
  linkKeys(d: Data, w: number) {
    const n = Math.min(128, d.sliceMode === "off" ? 25 : Math.max(12, SMP.cuts(d).length + 4)),
      lo = clamp(d.sliceMode === "off" ? d.root - 12 : d.root - 2, 0, 128 - n);
    return { n, lo, kw: w / n, kh: 22 };
  },
  /** 帯の形: 波形の区間(上の辺)と鍵盤(下の辺) */
  funnels(d: Data, w: number, h: number) {
    const K = SMP.linkKeys(d, w),
      cuts = SMP.cuts(d),
      ky = h - K.kh,
      kx = (m: number) => (m - K.lo) * K.kw;
    if (d.sliceMode === "off") return [{ si: null, m: d.root, a: d.start * w, b: d.end * w, k0: kx(d.root) + 1, k1: kx(d.root) + K.kw - 1, ky }];
    return cuts.map((c: number, i: number) => ({ si: i, m: d.root + i, a: c * w, b: (cuts[i + 1] ?? d.end) * w, k0: kx(d.root + i) + 1, k1: kx(d.root + i) + K.kw - 1, ky }));
  },
  /** つながりの絵の上の点 → 鍵盤(と区分)。帯の中なら、その帯の区分 */
  linkHit(d: Data, x: number, y: number, w: number, h: number) {
    const K = SMP.linkKeys(d, w),
      ky = h - K.kh;
    if (y >= ky) {
      const m = K.lo + clamp(Math.floor(x / K.kw), 0, K.n - 1),
        si = d.sliceMode === "off" ? null : m - d.root;
      return d.sliceMode !== "off" && si != null && (si < 0 || si >= SMP.cuts(d).length) ? { m, si: null, none: true } : { m, si };
    }
    const t = y / ky;
    const f = SMP.funnels(d, w, h).find((q: any) => x >= q.a + (q.k0 - q.a) * t && x <= q.b + (q.k1 - q.b) * t);
    return f ? { m: f.m, si: f.si } : null;
  },
  /** 区分(か素材全体)を 1 回鳴らす */
  audition(si: number | null) {
    const d = cur();
    const v = playNotes([si != null && d.sliceMode !== "off" ? d.root + si : d.root]);
    setTimeout(() => v.stop(), 450);
  },
  draw() {
    const d = shown();
    if (!d) return;
    const c = byId<HTMLCanvasElement>("smpWave");
    if (!c) return;
    const { g, w, h } = ctx2d(c);
    // いちばん下は時間の物差し
    const x = SMP.wave(d.src),
      top = 16,
      bot = h - 30,
      ry = h - 15;
    const cuts = SMP.cuts(d),
      tool = toolOf("wave");
    // 押している鍵盤の区分(水色)
    if (SMP.lit != null && cuts.length) {
      const li = SMP.lit - d.root;
      if (li >= 0 && li < cuts.length) {
        g.fillStyle = "rgba(37,189,177,0.25)";
        g.fillRect(cuts[li] * w, top, ((cuts[li + 1] ?? d.end) - cuts[li]) * w, bot - top);
      }
    }
    // 選んだ区分(白)
    if (d.selSlice != null && cuts.length && d.selSlice < cuts.length) {
      const a = cuts[d.selSlice],
        b = cuts[d.selSlice + 1] ?? d.end;
      g.fillStyle = "rgba(255,255,255,0.08)";
      g.fillRect(a * w, top, (b - a) * w, bot - top);
      g.strokeStyle = "#fff";
      g.lineWidth = 2;
      g.strokeRect(a * w + 1, top, (b - a) * w - 2, bot - top);
    }
    // ループ(区分のときはループしないので描かない)
    const loopOn = d.loop && d.sliceMode === "off";
    if (loopOn) {
      g.fillStyle = "rgba(37,189,177,0.13)";
      g.fillRect(d.ls * w, top, (d.le - d.ls) * w, bot - top);
      const xf = (d.xf / (SMP.len(d) * 1000)) * w;
      g.fillStyle = "rgba(37,189,177,0.35)";
      g.beginPath();
      g.moveTo(d.le * w - xf, bot);
      g.lineTo(d.le * w, top);
      g.lineTo(d.le * w, bot);
      g.fill();
    }
    if (x) drawAudio(g, x, w, top, bot);
    else {
      g.fillStyle = css("--faint");
      g.font = "11px sans-serif";
      g.fillText(d.src ? tr("素材を読み込んでいます…", "Loading the source…") : tr("素材がありません(左で選ぶか、ファイルから選ぶ)", "No source (pick one on the left or choose a file)"), 8, (top + bot) / 2);
    }
    // 使わない所
    g.fillStyle = "rgba(0,0,0,0.62)";
    g.fillRect(0, 0, d.start * w, h);
    g.fillRect(d.end * w, 0, w - d.end * w, h);
    // 区分の区間を、その区分の色で薄く塗る(下の帯・鍵盤と同じ色)
    cuts.forEach((cx: number, i: number) => {
      g.fillStyle = SMP.sliceColor(i, 0.1);
      g.fillRect(cx * w, top, ((cuts[i + 1] ?? d.end) - cx) * w, bot - top);
    });
    // 使う所の外の切れ目: 使わない(灰色の点線)。消さずに残すので、使う所を広げると戻る
    if (d.sliceMode !== "off")
      d.cuts
        .filter((cc: number) => !SMP.inUse(d, [cc]).length)
        .forEach((cx: number) => {
          const picked = tool === "cut" && d.selCut != null && d.cuts[d.selCut] === cx;
          g.strokeStyle = picked ? "#fff" : "rgba(160,160,160,0.6)";
          g.lineWidth = picked ? 2 : 1;
          g.setLineDash([3, 3]);
          g.beginPath();
          g.moveTo(cx * w, top);
          g.lineTo(cx * w, bot);
          g.stroke();
          g.setLineDash([]);
        });
    // 切れ目(区分 1 の頭 = 使う所の始まり)
    cuts.forEach((cx: number, i: number) => {
      const edit = tool === "cut" && i > 0,
        picked = edit && d.selCut != null && d.cuts[d.selCut] === cx;
      g.strokeStyle = d.selSlice === i || picked ? "#fff" : `rgba(232,196,106,${edit ? 0.95 : 0.7})`;
      g.lineWidth = picked ? 3 : edit ? 2 : 1;
      g.beginPath();
      g.moveTo(cx * w, top);
      g.lineTo(cx * w, bot);
      g.stroke();
      g.fillStyle = "rgba(232,196,106,0.95)";
      g.font = "10px sans-serif";
      g.fillText(String(i + 1), cx * w + 3, top + 11);
      if (edit) {
        g.beginPath();
        g.arc(cx * w, bot - 6, 4, 0, Math.PI * 2);
        g.fill();
      }
    });
    // つまみ(使う所 = 白 ▼ 上、ループ = 水色 ▲ 下)。今の道具のものだけ大きく
    const tri = (fx: number, col: string, label: string, up: boolean, big: boolean) => {
      const xx = fx * w,
        s = big ? 8 : 5,
        yb = ry - 1;
      g.fillStyle = col;
      g.beginPath();
      if (up) {
        g.moveTo(xx - s, yb);
        g.lineTo(xx + s, yb);
        g.lineTo(xx, yb - s * 1.5);
      } else {
        g.moveTo(xx - s, 0);
        g.lineTo(xx + s, 0);
        g.lineTo(xx, s * 1.5);
      }
      g.fill();
      g.fillRect(xx - 0.5, 0, 1, yb);
      if (label) {
        g.font = "10px sans-serif";
        g.fillText(label, clamp(xx + 5, 0, w - 50), up ? yb - 3 : 11);
      }
    };
    timeRuler(g, 0, w, ry, SMP.len(d), tr(" 秒", " s"));
    tri(d.start, "#e6e6e6", tool === "trim" ? tr("始まり", "Start") : "", false, tool === "trim");
    tri(d.end, "#e6e6e6", tool === "trim" ? tr("終わり", "End") : "", false, tool === "trim");
    if (loopOn) {
      tri(d.ls, css("--accent"), tool === "loop" ? tr("ループ", "Loop") : "", true, tool === "loop");
      tri(d.le, css("--accent"), "", true, tool === "loop");
    }
    if (d.selSlice != null && cuts.length && d.selSlice < cuts.length) tag(g, tr(`区分 ${d.selSlice + 1}(${noteName(d.root + d.selSlice)} で鳴る)`, `Slice ${d.selSlice + 1} (plays on ${noteName(d.root + d.selSlice)})`), cuts[d.selSlice] * w + 4, bot - 6, w);
    const info = byId("smpInfo");
    if (info) {
      const len = SMP.len(d);
      if (SMP.hoverX != null) {
        const outside = SMP.hoverX < d.start || SMP.hoverX > d.end,
          si = outside ? -1 : cuts.filter((cc: number) => cc <= SMP.hoverX).length - 1;
        info.textContent = tr(
          `${(SMP.hoverX * len).toFixed(2)} 秒${si >= 0 ? ` · 区分 ${si + 1}` : ""}${outside ? " · 使わない所" : ""}${tool === "cut" ? (d.selCut != null ? " · Delete・右クリックで消す" : " · クリックで線を足す") : ""}`,
          `${(SMP.hoverX * len).toFixed(2)} s${si >= 0 ? ` · slice ${si + 1}` : ""}${outside ? " · unused" : ""}${tool === "cut" ? (d.selCut != null ? " · Delete / right-click to remove" : " · click to add a line") : ""}`,
        );
      } else if (tool === "cut")
        info.textContent =
          d.selCut != null
            ? tr(`選んだ切れ目 ${(d.cuts[d.selCut] * len).toFixed(2)} 秒 · Delete・右クリックで消す`, `Selected cut ${(d.cuts[d.selCut] * len).toFixed(2)} s · Delete / right-click to remove`)
            : tr("線をつまむ · クリックで足す · Delete・右クリックで消す", "Drag a line · click to add · Delete / right-click to remove");
      else
        info.textContent = tr(
          `${len.toFixed(1)} 秒 · 使う所 ${(d.start * len).toFixed(2)}〜${(d.end * len).toFixed(2)} 秒${cuts.length ? ` · 区分 ${cuts.length}` : ""}${d.sliceMode !== "off" && SMP.handCut(d) ? " · 手で動かした線あり" : ""}`,
          `${len.toFixed(1)} s · used ${(d.start * len).toFixed(2)}–${(d.end * len).toFixed(2)} s${cuts.length ? ` · ${cuts.length} slices` : ""}${d.sliceMode !== "off" && SMP.handCut(d) ? " · has hand-moved lines" : ""}`,
        );
    }
    // 鍵盤へのつながり: 区分(か素材全体)→ それを鳴らす鍵盤
    const lc = byId<HTMLCanvasElement>("smpLink");
    if (lc) {
      const { g: g3, w: w3, h: h3 } = ctx2d(lc);
      const K = SMP.linkKeys(d, w3),
        ky = h3 - K.kh,
        off = d.sliceMode === "off",
        hv = SMP.linkHover;
      // 帯
      SMP.funnels(d, w3, h3).forEach((q: any) => {
        const sel = q.si != null && d.selSlice === q.si,
          lit = SMP.lit === q.m,
          hov = hv && hv.m === q.m;
        g3.beginPath();
        g3.moveTo(q.a, 0);
        g3.lineTo(q.b, 0);
        g3.lineTo(q.k1, q.ky);
        g3.lineTo(q.k0, q.ky);
        g3.closePath();
        g3.fillStyle = lit ? "rgba(37,189,177,0.45)" : off ? "rgba(37,189,177,0.16)" : SMP.sliceColor(q.si, sel || hov ? 0.42 : 0.22);
        g3.fill();
        g3.strokeStyle = sel ? "#fff" : off ? "rgba(37,189,177,0.6)" : SMP.sliceColor(q.si, 0.75);
        g3.lineWidth = sel ? 2 : 1;
        g3.stroke();
      });
      // 鍵盤
      for (let i = 0; i < K.n; i++) {
        const m = K.lo + i,
          x0 = i * K.kw,
          black = [1, 3, 6, 8, 10].includes(m % 12),
          si = off ? null : m - d.root,
          has = off || (si != null && si >= 0 && si < cuts.length);
        g3.fillStyle = black ? "#1c1c1c" : "#30302e";
        g3.fillRect(x0 + 0.5, ky, K.kw - 1, K.kh);
        if (has && !off && si != null) {
          g3.fillStyle = SMP.sliceColor(si, d.selSlice === si ? 0.85 : 0.55);
          g3.fillRect(x0 + 0.5, ky, K.kw - 1, K.kh);
        }
        if (off && m === d.root) {
          g3.fillStyle = "rgba(37,189,177,0.6)";
          g3.fillRect(x0 + 0.5, ky, K.kw - 1, K.kh);
        }
        if (SMP.lit === m && has) {
          g3.fillStyle = "rgba(37,189,177,0.9)";
          g3.fillRect(x0 + 0.5, ky, K.kw - 1, K.kh);
        }
        if (d.selSlice === si && !off) {
          g3.strokeStyle = "#fff";
          g3.lineWidth = 2;
          g3.strokeRect(x0 + 1.5, ky + 1, K.kw - 3, K.kh - 2);
        }
        if (hv && hv.m === m) {
          g3.strokeStyle = css("--human");
          g3.lineWidth = 1.5;
          g3.strokeRect(x0 + 1, ky + 0.5, K.kw - 2, K.kh - 1);
        }
        g3.font = "10px sans-serif";
        g3.textAlign = "center";
        // 区分の番号は帯の下の端
        if (!off && has && si != null && K.kw >= 12) {
          g3.fillStyle = "#fff";
          g3.fillText(String(si + 1), x0 + K.kw / 2, ky - 4);
        }
        // 区分のある鍵盤は全部に音の名前
        const named = m % 12 === 0 || (off && m === d.root) || (!off && has && K.kw >= 26);
        if (named) {
          g3.fillStyle = (off && m === d.root) || (!off && has) ? "#111" : css("--faint");
          g3.fillText(noteName(m), x0 + K.kw / 2, ky + K.kh - 6);
        }
        g3.textAlign = "left";
      }
      // 1 つの音のとき: 鍵盤ごとの速さ(高い鍵盤ほど速く短く・低いほど遅く長く)
      if (off) {
        g3.font = "10px sans-serif";
        g3.textAlign = "center";
        if (d.keyTrack)
          (
            [
              [-12, tr("×0.5(遅く・長く)", "×0.5 (slower, longer)")],
              [0, tr("×1 録音どおり", "×1 as recorded")],
              [12, tr("×2(速く・短く)", "×2 (faster, shorter)")],
            ] as [number, string][]
          ).forEach(([dm, t]) => {
            const x0 = (d.root + dm - K.lo + 0.5) * K.kw;
            g3.fillStyle = dm ? css("--dim") : css("--accent");
            g3.fillText(t, clamp(x0, 50, w3 - 50), ky - 5);
          });
        else {
          g3.fillStyle = css("--dim");
          g3.fillText(tr("どの鍵盤も録音どおりの速さで鳴る(鍵盤で高さを変えるが切ってある)", "Every key plays at the recorded speed (key tracking is off)"), w3 / 2, ky - 5);
        }
        g3.textAlign = "left";
      }
    }
    // 下の鍵盤(本物の形): 区分の色・録音どおりの鍵盤・今鳴っている鍵盤。押すと鳴る
    const pc = byId<HTMLCanvasElement>("smpPiano");
    if (pc) {
      const { g: g4, w: w4, h: h4 } = ctx2d(pc),
        off = d.sliceMode === "off",
        ph = SMP.pianoHover;
      const K = SMP.pianoKeys(d, w4, h4);
      K.forEach((k: any) => {
        const si = off ? null : k.m - d.root,
          has = !off && si != null && si >= 0 && si < cuts.length;
        g4.fillStyle = k.black ? "#161616" : "#d8d6d0";
        if (has && si != null) g4.fillStyle = SMP.sliceColor(si, k.black ? 0.95 : 0.9);
        if (off && k.m === d.root) g4.fillStyle = "rgba(37,189,177,0.95)";
        if (SMP.lit === k.m && (off || has)) g4.fillStyle = "#25bdb1";
        g4.fillRect(k.x + 0.5, k.y, k.w - 1, k.h - (k.black ? 0 : 1));
        g4.strokeStyle = "#111";
        g4.lineWidth = 1;
        g4.strokeRect(k.x + 0.5, k.y + 0.5, k.w - 1, k.h - 1);
        if (has && d.selSlice === si) {
          g4.strokeStyle = "#fff";
          g4.lineWidth = 2;
          g4.strokeRect(k.x + 1.5, k.y + 1.5, k.w - 3, k.h - 3);
        }
        if (ph === k.m) {
          g4.strokeStyle = css("--human");
          g4.lineWidth = 1.5;
          g4.strokeRect(k.x + 1, k.y + 1, k.w - 2, k.h - 2);
        }
        if (!k.black) {
          g4.font = "9px sans-serif";
          g4.textAlign = "center";
          g4.fillStyle = "#222";
          if (has && si != null && k.w >= 10) g4.fillText(String(si + 1), k.x + k.w / 2, h4 - 16);
          if (k.m % 12 === 0 || (off && k.m === d.root)) g4.fillText(noteName(k.m), k.x + k.w / 2, h4 - 4);
          g4.textAlign = "left";
        } else if (has && si != null && k.w >= 8) {
          // 黒鍵の番号は上寄せ(白鍵の番号と重ならないように)
          g4.font = "9px sans-serif";
          g4.textAlign = "center";
          g4.fillStyle = "#111";
          g4.fillText(String(si + 1), k.x + k.w / 2, Math.min(k.h - 4, 11));
          g4.textAlign = "left";
        }
      });
      const pi = byId("smpPianoInfo");
      if (pi) {
        const m = ph;
        if (m == null) pi.textContent = off ? tr("押すと鳴る · 水色 = 録音どおり", "Press to play · teal = as recorded") : tr("押すと鳴る · 色 = 区分", "Press to play · color = slice");
        else {
          const si = m - d.root,
            dm = m - d.root;
          pi.textContent = off
            ? d.keyTrack
              ? tr(`${noteName(m)} · ${dm >= 0 ? "+" : ""}${dm} 半音(×${Math.pow(2, dm / 12).toFixed(2)} の速さ)`, `${noteName(m)} · ${dm >= 0 ? "+" : ""}${dm} st (×${Math.pow(2, dm / 12).toFixed(2)} speed)`)
              : tr(`${noteName(m)} · 録音どおりの速さ`, `${noteName(m)} · recorded speed`)
            : si >= 0 && si < cuts.length
              ? tr(`${noteName(m)} · 区分 ${si + 1}`, `${noteName(m)} · slice ${si + 1}`)
              : tr(`${noteName(m)} · 鳴らない(区分が無い)`, `${noteName(m)} · silent (no slice)`);
        }
      }
    }
    // マウスの下(つながりの絵): どの鍵盤で何が鳴るか(1 行に収まる長さ。折り返すと下の部品が動く)
    if (info && SMP.linkHover) {
      const L = SMP.linkHover,
        len = SMP.len(d),
        dm = L.m - d.root;
      info.textContent = L.none
        ? tr(`${noteName(L.m)} · 区分が無い(鳴らない)`, `${noteName(L.m)} · no slice (silent)`)
        : L.si != null
          ? tr(`${noteName(L.m)} · 区分 ${L.si + 1} · ${(cuts[L.si] * len).toFixed(2)}〜${((cuts[L.si + 1] ?? d.end) * len).toFixed(2)} 秒`, `${noteName(L.m)} · slice ${L.si + 1} · ${(cuts[L.si] * len).toFixed(2)}–${((cuts[L.si + 1] ?? d.end) * len).toFixed(2)} s`)
          : d.keyTrack
            ? tr(`${noteName(L.m)} · ${dm >= 0 ? "+" : ""}${dm} 半音 · ×${Math.pow(2, dm / 12).toFixed(2)} の速さ`, `${noteName(L.m)} · ${dm >= 0 ? "+" : ""}${dm} st · ×${Math.pow(2, dm / 12).toFixed(2)} speed`)
            : tr(`${noteName(L.m)} · 録音どおりの速さ`, `${noteName(L.m)} · recorded speed`);
    }
    ui.panes.forEach((p) => p.draw());
  },
  // 鳴らしても選択は変えない(光らせるだけ)
  onKey(m: number, on: boolean) {
    SMP.lit = on ? m : null;
    SMP.draw();
  },
  refresh() {
    rerender();
  },
  help: (): [string, string][] => [
    [tr("素材", "Source"), tr("左で選ぶ。曲の中の音声クリップも使える。「ファイルから選ぶ…」で読み込める", "Pick on the left. Audio clips in the song work too. Load a file with “Choose a file…”")],
    [tr("見るだけ", "View"), tr("区分をクリックで選んで鳴らす(白く囲まれる)", "Click a slice to select and play it (outlined in white)")],
    [tr("使う所を切る", "Trim"), tr("白い ▼ をつまんで、始まりと終わりを決める(暗い所は鳴らない)", "Drag the white ▼ marks to set the start and end (dark parts don't play)")],
    [tr("ループを決める", "Set the loop"), tr("水色の ▲ をつまむ。右端の濃い三角がつなぎ目(長さは「つなぎ目」で)", "Drag the teal ▲ marks. The dark triangle at the right is the crossfade (length via “Crossfade”)")],
    [tr("切れ目を調整", "Edit cuts"), tr("黄色の線をつまんで動かす。何も無い所をクリックで足す。線をクリックして Delete キー、または右クリックで消す", "Drag the yellow lines. Click an empty spot to add one. Select a line and press Delete, or right-click it, to remove it")],
    [tr("鍵盤へのつながり", "Key links"), tr("波形の下の帯で、区分(か素材全体)とそれを鳴らす鍵盤をつなぐ。鍵盤か帯を押している間鳴る", "The bands under the waveform link each slice (or the whole source) to its key. Press a key or band to play while held")],
    [
      tr("鍵盤での鳴らし方", "How keys play it"),
      tr("「1 つの音として高さを変えて弾く」か「区分に分けて鍵盤に並べる」。区分の線は上の「切れ目を調整」でいつでも動かせる。「音の頭で切り直す」で自動で引き直す。下の鍵盤は押している間鳴る", "“Play as one sound” or “Split into slices”. Move slice lines any time with “Edit cuts” above; “Re-slice at onsets” redraws them automatically. The keyboard below plays while pressed"),
    ],
    [tr("フィルタ・音量の変わり方", "Filter · level over time"), tr("「点をつまむ」で ○ を動かす", "Use “Drag points” to move the ○")],
  ],
};

register(SMP);
export default SMP;
