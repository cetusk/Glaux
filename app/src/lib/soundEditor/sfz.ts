// 音色エディタ: SFZ。録音を鍵盤に割り当てた音源(ドラムキット・ピアノなど)を、鍵盤 1 つずつ調整する。
// 左 = この SFZ が用意している調整つまみ(キット全体に効く)・使っている SFZ・保存、
// 上 = ドラムなら上から見たキットの絵(割り当ての鍵盤から自動で並べる)、ドラムでなければ鍵盤の図。クリックで叩いて選ぶ、
// 下 = 選んだ鍵盤の録音の波形と、音量・音程・長さ・強さの効き方(エンジンの key_adjust)
import * as api from "../api";
import { tr } from "../i18n.svelte";
import { instrumentPickerStore } from "../selection.svelte";
import { draw as drawAll, playNotes, register, setSel, type Data, type Loaded } from "./core.svelte";
import { byId, clamp, css, ctx2d, drawAudio, el, noteName, timeRuler } from "./dom";
import { canEdit, cur, editHead, knob, knobLabel, legendRow, pane, pushHist, rerender, saveBox, shelfList, shown, specBox, state, toolSeg, ui } from "./parts";

// ---- ドラムの標準の並び(GM)で、鍵盤 → 何の物・どこを叩くか。SFZ の割り当てからキットの絵を組み立てる手がかり ----
// [物の id, 物の名前, 英語の名前, 種類, 叩く所, 英語の叩く所]。種類: kick・snare・tom(上に並ぶタム)・floor(フロアタム)・hat・crash・ride・perc(打楽器)
type Gm = [string, string, string, string, string, string];
const GM_DRUM: Record<number, Gm> = {
  35: ["kick", "キック", "Kick", "kick", "2 つ目", "2nd"],
  36: ["kick", "キック", "Kick", "kick", "1 つ目", "1st"],
  37: ["snare", "スネア", "Snare", "snare", "クロススティック", "cross-stick"],
  38: ["snare", "スネア", "Snare", "snare", "真ん中", "center"],
  40: ["snare", "スネア", "Snare", "snare", "ふち", "rim"],
  41: ["floor2", "フロアタム(低)", "Floor tom (low)", "floor", "", ""],
  43: ["floor1", "フロアタム(高)", "Floor tom (high)", "floor", "", ""],
  45: ["tom3", "タム(低)", "Tom (low)", "tom", "", ""],
  47: ["tom2", "タム(中)", "Tom (mid)", "tom", "", ""],
  48: ["tom1", "タム(高)", "Tom (high)", "tom", "", ""],
  50: ["tom0", "タム(最高)", "Tom (highest)", "tom", "", ""],
  42: ["hat", "ハイハット", "Hi-hat", "hat", "閉", "closed"],
  44: ["hat", "ハイハット", "Hi-hat", "hat", "ペダル", "pedal"],
  46: ["hat", "ハイハット", "Hi-hat", "hat", "開", "open"],
  49: ["crash1", "クラッシュ", "Crash", "crash", "", ""],
  57: ["crash2", "クラッシュ 2", "Crash 2", "crash", "", ""],
  55: ["splash", "スプラッシュ", "Splash", "crash", "", ""],
  52: ["china", "チャイナ", "China", "crash", "", ""],
  51: ["ride", "ライド", "Ride", "ride", "面", "bow"],
  53: ["ride", "ライド", "Ride", "ride", "カップ", "bell"],
  59: ["ride", "ライド", "Ride", "ride", "ふち", "edge"],
  39: ["clap", "手拍子", "Clap", "perc", "", ""],
  54: ["tamb", "タンバリン", "Tambourine", "perc", "", ""],
  56: ["cowbell", "カウベル", "Cowbell", "perc", "", ""],
};
const SHORT = (): Record<string, string> => ({
  floor1: tr("フロア高", "Floor hi"),
  floor2: tr("フロア低", "Floor lo"),
  tom0: tr("タム最高", "Tom top"),
  tom1: tr("タム高", "Tom hi"),
  tom2: tr("タム中", "Tom mid"),
  tom3: tr("タム低", "Tom lo"),
  crash1: tr("クラッシュ", "Crash"),
  crash2: tr("クラッシュ2", "Crash 2"),
});
const PIECE_DEF = { vol: 0, tune: 0, len: 100, vt: 100 };
type Piece = typeof PIECE_DEF;
type Part = { id: string; name: string; kind: string; keys: { key: number; art: string }[] };
const IN = 7; // 1 インチの長さ(絵の単位)

/** 楽器の中身(SFZ ごと) */
const infos: Record<string, api.SfzInspect | { error: string }> = {};
/** 鍵盤の録音の波形(SFZ・つまみ・鍵盤・強さの段ごと) */
const waves = new Map<string, { peaks: number[]; seconds: number; sample: string } | "loading" | "error">();
/** 置き場所(SFZ のフォルダ) */
let sfzDir = "";

const ccOverrides = (d: Data): Record<string, number> => {
  const info = SFZ.info(d);
  const out: Record<string, number> = {};
  for (const c of info?.controls ?? []) {
    const v = d.cc[c.cc];
    if (typeof v === "number" && v !== c.default) out[String(c.cc)] = v;
  }
  return out;
};

const SFZ: any = {
  kind: "sfz",
  title: () => "SFZ",
  async load({ params, track }: Loaded): Promise<Data> {
    const dv: any = track.device ?? {};
    const instrument: string = dv.instrument ?? "";
    if (instrument && !infos[instrument])
      infos[instrument] = await api.sfzInspect(instrument, {}).catch((e) => ({ error: String(e) }));
    if (!sfzDir)
      api
        .listSoundfonts()
        .then((r) => (sfzDir = r.sfz_dir ?? ""))
        .catch(() => undefined);
    const info = infos[instrument];
    const cc: Record<number, number> = {};
    if (info && "controls" in info) for (const c of info.controls) cc[c.cc] = c.default;
    for (const [k, v] of Object.entries(dv.cc ?? {})) if (typeof v === "number") cc[+k] = v;
    const adj: Record<number, Piece> = {};
    for (const item of String(params.key_adjust ?? "").split(",")) {
      const f = item.trim().split(":");
      const key = parseInt(f[0]);
      if (!Number.isFinite(key) || key < 0 || key > 127) continue;
      const n = (i: number, def: number) => (Number.isFinite(parseFloat(f[i])) ? parseFloat(f[i]) : def);
      adj[key] = { vol: n(1, 0), tune: n(2, 0), len: n(3, 100), vt: n(4, 100) };
    }
    return { instrument, cc, adj, gain: typeof params.gain_db === "number" ? params.gain_db : 0 };
  },
  uiInit: (): Data => ({ sel: null, hit: {}, playing: null, showShelf: false }),
  params: (d: Data) => ({
    gain_db: d.gain,
    key_adjust: Object.keys(d.adj)
      .map(Number)
      .sort((a, b) => a - b)
      .filter((k) => SFZ.edited(d, k))
      .map((k) => {
        const p = SFZ.pieceOf(d, k);
        return `${k}:${p.vol}:${p.tune}:${p.len}:${p.vt}`;
      })
      .join(","),
  }),
  /** 調整つまみ(CC)は音源の差し替えで当てる(つまみの値はそのまま) */
  extraCommands(trackId: string, prev: Data, next: Data) {
    const a = JSON.stringify(ccOverrides(prev)),
      b = JSON.stringify(ccOverrides(next));
    if (a === b || !next.instrument) return [];
    const dv: any = state.project?.tracks.find((t) => t.id === trackId)?.device ?? {};
    const cc = ccOverrides(next);
    return [{ op: "set_device", track: trackId, device: { type: "sfz", instrument: next.instrument, ...(Object.keys(cc).length ? { cc } : {}), params: { ...(dv.params ?? {}), ...SFZ.params(next) } } }];
  },
  info(d: Data): api.SfzInspect | null {
    const i = infos[d?.instrument];
    return i && "keys" in i ? i : null;
  },
  /** 鍵盤の多くがドラムの標準の並びにあればキットの絵、無ければ鍵盤の図 */
  isKit(d: Data) {
    const info = SFZ.info(d);
    if (!info?.keys.length) return false;
    const gm = info.keys.filter((k: any) => GM_DRUM[k.key]).length;
    return gm >= 3 && gm >= info.keys.length * 0.6;
  },
  /** 割り当てから物(パーツ)を組み立てる: 同じ物の叩き分けは 1 つにまとめる。標準の並びに無い鍵盤は打楽器のパッド */
  pieces(d: Data): Part[] {
    const info = SFZ.info(d);
    const m = new Map<string, Part>();
    for (const k of info?.keys ?? []) {
      const g = GM_DRUM[k.key];
      const id = g ? g[0] : `k${k.key}`;
      if (!m.has(id)) m.set(id, { id, name: g ? tr(g[1], g[2]) : noteName(k.key), kind: g ? g[3] : "perc", keys: [] });
      m.get(id)!.keys.push({ key: k.key, art: g ? tr(g[4], g[5]) : "" });
    }
    return [...m.values()];
  },
  partOf(d: Data, key: number): Part | null {
    return SFZ.pieces(d).find((p: Part) => p.keys.some((k) => k.key === key)) ?? null;
  },
  /** 名前: 叩き分けのある物は「スネア(ふち)」、無い物は「タム(高)」。キットでなければ音の名前 */
  label(d: Data, key: number) {
    if (!SFZ.isKit(d)) return noteName(key);
    const part = SFZ.partOf(d, key),
      art = part?.keys.find((k: any) => k.key === key)?.art;
    return part ? (part.keys.length > 1 && art ? tr(`${part.name}(${art})`, `${part.name} (${art})`) : part.name) : noteName(key);
  },
  pieceOf: (d: Data, key: number): Piece => ({ ...PIECE_DEF, ...(d.adj[key] ?? {}) }),
  edited: (d: Data, key: number) => {
    const p = d.adj[key];
    return !!p && (Object.keys(PIECE_DEF) as (keyof Piece)[]).some((k) => p[k] !== PIECE_DEF[k]);
  },
  /** 選んでいる鍵盤(無ければ、キットならスネア → 最初の鍵盤) */
  selKey(d: Data): number | null {
    const keys = SFZ.info(d)?.keys.map((k: any) => k.key) ?? [];
    if (d.sel != null && keys.includes(d.sel)) return d.sel;
    return keys.includes(38) && SFZ.isKit(d) ? 38 : (keys[0] ?? null);
  },
  keyInfo: (d: Data, key: number) => SFZ.info(d)?.keys.find((k: any) => k.key === key) ?? null,
  /** その鍵盤の叩く強さ(絵や鍵盤で叩くときに使う。曲で鳴る音は曲のノートの強さ) */
  hitOf: (d: Data, key: number) => (d.hit ?? {})[key] ?? 100,
  /** 鳴っている録音の波形(強さの段ごと)。届くまでは null */
  wave(d: Data, key: number, vel: number) {
    const k = SFZ.keyInfo(d, key);
    const li = k?.layers.findIndex((l: api.SfzLayer) => vel >= l.vel_lo && vel <= l.vel_hi) ?? -1;
    const id = JSON.stringify([d.instrument, ccOverrides(d), key, li]);
    const w = waves.get(id);
    if (w === "loading" || w === "error") return null;
    if (w) return w;
    waves.set(id, "loading");
    api
      .sfzKeyWave(d.instrument, ccOverrides(d), key, vel, 0, 900)
      .then((r) => {
        waves.set(id, { peaks: r.peaks.flat(), seconds: r.seconds, sample: r.sample });
        if (state.inst === "sfz") SFZ.draw();
      })
      .catch(() => waves.set(id, "error"));
    return null;
  },
  play(key: number, vel: number) {
    const d = cur();
    const v = playNotes([key], vel);
    d.playing = key;
    SFZ.draw();
    setTimeout(() => {
      if (cur()?.playing === key) {
        cur().playing = null;
        SFZ.draw();
      }
      v.stop();
    }, 300);
  },
  render(body: HTMLElement) {
    const d = cur();
    body.innerHTML = "";
    const info = SFZ.info(d);
    if (!info) {
      const e = infos[d.instrument];
      body.append(
        el(
          "div",
          { class: "col center" },
          el(
            "div",
            { class: "box" },
            el("h3", {}, tr("SFZ を読み込めません", "Couldn't read the SFZ")),
            el("div", { class: "hint" }, e && "error" in e ? e.error : tr("音源が決まっていません", "No instrument is set")),
            el("button", { class: "btn", onclick: (ev: MouseEvent) => SFZ.pick(ev) }, tr("SFZ を替える…", "Change SFZ…")),
          ),
        ),
      );
      return;
    }
    const key: number | null = SFZ.selKey(d);
    const kit = SFZ.isKit(d);
    const part: Part | null = key != null ? SFZ.partOf(d, key) : null;
    const kinfo = key != null ? SFZ.keyInfo(d, key) : null;
    // ---- 左: キット全体の調整・使っている SFZ・保存 ----
    const left = el("div", { class: "col left", style: "width: 260px" });
    const srcSeg = el("div", { class: "seg" });
    (
      [
        ["knobs", tr("つまみ", "Controls")],
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
    const whole = kit ? tr("キット全体の調整", "Whole-kit controls") : tr("音源全体の調整", "Instrument controls");
    const ctl = el(
      "div",
      { class: "box" },
      el(
        "h3",
        {},
        whole,
        el("span", { class: "spacer" }),
        info.controls.length
          ? el(
              "button",
              {
                class: "btn quiet",
                onclick: () => {
                  if (!canEdit()) return;
                  for (const c of info.controls) d.cc[c.cc] = c.default;
                  pushHist(tr(`${whole}を既定に戻す`, `Reset the ${knobLabel(whole).toLowerCase()}`));
                  rerender();
                },
              },
              tr("既定に戻す", "Reset"),
            )
          : null,
      ),
      srcSeg,
    );
    if (d.showShelf) ctl.append(shelfList("sfz"));
    else if (!info.controls.length) ctl.append(el("div", { class: "hint" }, tr("この SFZ には調整つまみがありません", "This SFZ has no controls")));
    else {
      const name = key != null ? SFZ.label(d, key) : "";
      ctl.append(
        el(
          "div",
          { class: "hint" },
          kit
            ? tr(`この SFZ が用意しているつまみ。キット全部に効きます。選んでいる「${name}」に効くものは水色。`, `Controls this SFZ provides. They affect the whole kit; the ones that affect “${name}” are teal.`)
            : tr(`この SFZ が用意しているつまみ。音源全部に効きます。選んでいる ${name} に効くものは水色。`, `Controls this SFZ provides. They affect the whole instrument; the ones that affect ${name} are teal.`),
        ),
      );
      for (const c of info.controls)
        ctl.append(
          knob(c.label, {
            min: 0,
            max: 127,
            step: 1,
            value: d.cc[c.cc] ?? c.default,
            icon: "",
            title: tr(`CC ${c.cc} · 既定 ${c.default}`, `CC ${c.cc} · default ${c.default}`),
            cls: kinfo?.ccs.includes(c.cc) ? "ch" : "",
            bind: `${state.track}:cc${c.cc}`,
            onInput: (v) => {
              d.cc[c.cc] = v;
            },
            onCommit: (v) => {
              pushHist(tr(`${c.label}を ${v} に`, `${c.label} → ${v}`));
              setSel(tr(`つまみ「${c.label}」`, `Control “${c.label}”`));
            },
          }),
        );
    }
    left.append(ctl);
    const pieces: Part[] = SFZ.pieces(d);
    const stacked = Math.max(1, ...info.keys.flatMap((k: any) => k.layers.map((l: api.SfzLayer) => l.stack)));
    left.append(
      el(
        "div",
        { class: "box" },
        el("h3", {}, tr("使っている SFZ", "SFZ in use")),
        el("div", { class: "small", style: "overflow-wrap: anywhere" }, d.instrument.replace(/\.sfz$/i, "")),
        el("div", { class: "hint", title: sfzDir }, tr("置き場所: SFZ のフォルダ(全部の曲で共通)", "Location: the SFZ folder (shared by every song)")),
        el(
          "div",
          { class: "hint", title: tr("どの鍵盤・強さでどの録音を鳴らすかは、この SFZ のファイルで決まっている", "Which recording plays for each key and strength is set by the SFZ file") },
          kit
            ? tr(`中身: ${pieces.length} 個のパーツ · ${info.keys.length} 音 · 録音 ${info.samples} 個${stacked > 1 ? ` · 重ねる録音 ${stacked} 本` : ""}`, `Contents: ${pieces.length} pieces · ${info.keys.length} keys · ${info.samples} recordings${stacked > 1 ? ` · ${stacked} layered` : ""}`)
            : tr(`中身: ${info.keys.length} 音(${noteName(info.keys[0]?.key ?? 0)}〜${noteName(info.keys[info.keys.length - 1]?.key ?? 0)})· 録音 ${info.samples} 個`, `Contents: ${info.keys.length} keys (${noteName(info.keys[0]?.key ?? 0)}–${noteName(info.keys[info.keys.length - 1]?.key ?? 0)}) · ${info.samples} recordings`),
        ),
        el(
          "div",
          { class: "row", style: "flex-wrap: wrap" },
          el("button", { class: "btn", title: tr("このトラックの音源を、SFZ のフォルダにある別の SFZ に替える", "Switch this track to another SFZ in the SFZ folder"), onclick: (ev: MouseEvent) => SFZ.pick(ev) }, tr("SFZ を替える…", "Change SFZ…")),
        ),
      ),
    );
    left.append(saveBox(state.trackName, kit ? tr("キット全体の調整と個別の調整", "the kit controls and per-piece adjustments") : tr("音源全体の調整と鍵盤ごとの調整", "the controls and per-key adjustments")));
    // ---- 真ん中 ----
    const center = el("div", { class: "col center wtcol", style: "overflow-y: auto" });
    const grid = el("div", { class: "wtgrid", style: "grid-template-rows: minmax(250px, 1.4fr) minmax(250px, 1fr)" });
    const map = el("canvas", { id: "sfzKit", class: "fill", style: "cursor: pointer" });
    const kInfo = el("div", { class: "paneinfo", id: "sfzInfo" });
    grid.append(
      el(
        "div",
        { class: "box", style: "grid-column: span 4" },
        el(
          "h3",
          { title: kit ? tr("SFZ の割り当て(鍵盤の番号。ドラムの標準の並び)から、何の音かを読んで自動で並べる", "Laid out automatically from the SFZ's key mapping (General MIDI drum layout)") : tr("SFZ が録音を割り当てている鍵盤", "Keys the SFZ maps recordings to") },
          kit ? tr("ドラムキット", "Drum kit") : tr("鍵盤の割り当て", "Key mapping"),
          el("span", { class: "spacer" }),
          legendRow([
            ["lit", tr("今鳴っている", "Sounding")],
            ["now", tr("下で調整しているもの", "Being adjusted below")],
            [el("span", { class: "sw dot" }), tr("個別に調整した", "Adjusted")],
            ...(kit ? ([["pad", tr("打楽器", "Percussion")]] as any) : []),
          ]),
        ),
        map,
        kInfo,
      ),
    );
    grid.append(specBox("grid-column: span 2"));
    // 下: 選んだ鍵盤を調整する。叩き分けのあるパーツは「叩く所」で選ぶ
    if (key != null) {
      const P: Piece = SFZ.pieceOf(d, key),
        nm = SFZ.label(d, key),
        hv = SFZ.hitOf(d, key);
      const wave = el("canvas", { id: "sfzSample", class: "fill", style: "cursor: pointer", title: tr("クリックで鳴らす", "Click to play") });
      const arts =
        kit && part && part.keys.length > 1
          ? toolSeg(
              part.keys.map((k) => [String(k.key), k.art || noteName(k.key), tr(`${part.name}の「${k.art}」(${noteName(k.key)} の鍵盤)。クリックで叩く`, `${part.name}: ${k.art} (key ${noteName(k.key)}). Click to hit`), "listen"]),
              String(key),
              (t) => {
                d.sel = +t;
                SFZ.play(+t, SFZ.hitOf(d, +t));
                rerender();
              },
            )
          : null;
      const k = (label: string, name: keyof Piece, min: number, max: number, step: number, unit: string, title?: string) =>
        knob(label, {
          min,
          max,
          step,
          value: P[name],
          unit,
          title,
          onInput: (v) => {
            d.adj[key] = { ...SFZ.pieceOf(d, key), [name]: v };
            SFZ.draw();
          },
          onCommit: (v) => {
            pushHist(tr(`${nm}の${label}を ${v}${unit} に`, `${nm}: ${knobLabel(label)} → ${v}${unit}`));
            rerender();
          },
        });
      const setHit = (v: number, commit: boolean) => {
        (d.hit ??= {})[key] = v;
        if (commit) rerender();
        else SFZ.draw();
      };
      const layers: api.SfzLayer[] = kinfo?.layers ?? [];
      const rrs = Math.max(1, ...layers.map((l) => l.rr));
      const bar = el("canvas", {
        id: "sfzLayers",
        style: "height: 42px; flex: none; cursor: pointer",
        title: tr(
          `強さで録音が ${layers.length} つに切り替わる(白い線 = 叩く強さ)${rrs > 1 ? `。続けて叩くと ${rrs} つを順番に使う` : ""}。クリックで、その強さにして叩く`,
          `The recording switches between ${layers.length} by strength (white line = hit strength)${rrs > 1 ? `; repeated hits cycle through ${rrs}` : ""}. Click to hit at that strength`,
        ),
      });
      bar.addEventListener("pointerdown", (ev) => {
        const r = bar.getBoundingClientRect();
        const v = clamp(Math.round(((ev.clientX - r.left - 6) / (r.width - 12)) * 127), 1, 127);
        setHit(v, true);
        SFZ.play(key, v);
      });
      const curve = el("canvas", { id: "sfzVelCurve", style: "height: 58px; flex: none", title: tr("横 = 叩く強さ、縦 = 音量", "Across = hit strength, up = level") });
      curve.addEventListener("pointermove", (ev) => {
        const r = curve.getBoundingClientRect();
        SFZ.hoverVel = clamp(Math.round(((ev.clientX - r.left - 30) / (r.width - 36)) * 127), 1, 127);
        SFZ.draw();
      });
      curve.addEventListener("pointerleave", () => {
        SFZ.hoverVel = null;
        SFZ.draw();
      });
      // 鳴らし方: つまみを 1 列に。絵(曲線・強さの帯)は、そのつまみのすぐ下
      const how = el(
        "div",
        { class: "howpane" },
        k("音量", "vol", -24, 12, 0.5, " dB"),
        k("音程", "tune", -12, 12, 0.1, tr(" 半音", " st")),
        k("長さ", "len", 10, 100, 1, " %", tr("後ろを消して短くする(100 % = 録音のまま)", "Shorten by fading out the end (100 % = as recorded)")),
        k(
          "強さの効き方",
          "vt",
          0,
          100,
          1,
          " %",
          tr("弱く叩いたとき、どれだけ小さくなるか(amp_veltrack)。0 % = 強さに関係なく同じ音量、100 % = SFZ のまま", "How much quieter soft hits get (amp_veltrack). 0 % = same level at any strength, 100 % = as in the SFZ"),
        ),
        curve,
        el("div", { class: "small dim", id: "sfzVelInfo", style: "min-height: 2lh" }, ""),
        knob("叩く強さ", {
          min: 1,
          max: 127,
          step: 1,
          value: hv,
          title: tr(`${nm}を絵や鍵盤で叩くときの強さ(ベロシティ)。鍵盤ごとに別の値。曲で鳴る音は、曲のノートの強さのまま`, `Strength (velocity) used when hitting ${nm} from the picture or keys. Separate per key. Notes in the song keep their own velocity`),
          onInput: (v) => setHit(v, false),
          onCommit: (v) => setHit(v, true),
        }),
        bar,
      );
      grid.append(
        el(
          "div",
          { class: "box editgroup", style: "grid-column: span 6" },
          editHead(
            kit && part ? tr(`${part.name}を調整`, `Adjust ${part.name}`) : tr(`${noteName(key)} を調整`, `Adjust ${noteName(key)}`),
            tr(`${noteName(key)} の鍵盤`, `Key ${noteName(key)}`),
            "",
            arts ? el("span", { class: "row small artpick" }, el("span", { class: "dim" }, tr("叩く所", "Hit spot")), arts) : null,
            SFZ.edited(d, key)
              ? el(
                  "button",
                  {
                    class: "btn quiet",
                    onclick: () => {
                      if (!canEdit()) return;
                      delete d.adj[key];
                      pushHist(tr(`${nm}の調整を元に戻す`, `Reset ${nm}`));
                      rerender();
                    },
                  },
                  tr("調整を元に戻す", "Reset adjustments"),
                )
              : null,
          ),
          el(
            "div",
            { class: "editpanes", style: "grid-template-columns: minmax(0, 1.2fr) minmax(0, 1fr)" },
            pane(tr("音の形", "Sound shape"), { canvas: wave, info: el("div", { id: "sfzWaveInfo", style: "min-height: 2lh" }) }),
            pane(tr(`${nm}の鳴らし方`, `How ${nm} plays`), { below: how, cls: "scroll" }),
          ),
        ),
      );
      wave.addEventListener("pointerdown", () => SFZ.play(key, SFZ.hitOf(d, key)));
    }
    center.append(grid);
    body.append(left, center);
    // ---- 絵: クリックで叩いて選ぶ ----
    const hitAt = (ev: PointerEvent) => {
      const r = map.getBoundingClientRect();
      return SFZ.mapHit(cur(), ev.clientX - r.left, ev.clientY - r.top, r.width, r.height);
    };
    map.addEventListener("pointerdown", (ev) => {
      const h: number[] | null = hitAt(ev);
      if (!h) return;
      const D = cur();
      const kk = h.includes(D.sel) ? D.sel : h[0];
      D.sel = kk;
      setSel(SFZ.label(D, kk));
      SFZ.play(kk, SFZ.hitOf(D, kk));
      rerender();
    });
    map.addEventListener("pointermove", (ev) => {
      SFZ.hover = hitAt(ev);
      map.style.cursor = SFZ.hover ? "pointer" : "default";
      SFZ.draw();
    });
    map.addEventListener("pointerleave", () => {
      SFZ.hover = null;
      SFZ.draw();
    });
  },
  /** 「SFZ を替える…」: インスペクターと同じ音源の選び方を開く */
  pick(ev: MouseEvent) {
    if (!canEdit()) return;
    const r = (ev.currentTarget as HTMLElement).getBoundingClientRect();
    instrumentPickerStore.open = { trackId: state.track, x: r.left, y: r.bottom + 4, tab: "sf2" };
  },
  hover: null as number[] | null,
  hoverVel: null as number | null,
  // ---- キットの並び(上から見た、右利きの並び。実物の寸法〈インチ〉で置く) ----
  /** 叩く人の椅子の真ん中を原点に、右が x、前(絵の上)が y。1 インチ = 7(絵の単位)。
   *  キック 22″×18″ は叩く人の右足の前から前へ伸びる胴、スネア 14″ は両膝の間、ハイハット 14″ はスネアの左、
   *  タムはキックの上、フロアタムはキックの右の床、ライドはフロアタムの前の上、シンバルは太鼓の上に重なる円。打楽器のパッドはキットの外(下) */
  layout(d: Data, step = 64) {
    const P: Part[] = SFZ.pieces(d),
      out: any[] = [];
    const by = (kind: string) => P.filter((p) => p.kind === kind);
    const put = (p: Part, x: number, y: number, r: number, extra: any = {}) => out.push({ ...p, x: x * IN, y: -y * IN, r: r * IN, ...extra });
    by("kick").forEach((p) => put(p, 2, 31, 0, { rect: [11 * IN, 9 * IN], lab: [2 * IN, -25.5 * IN] }));
    const floors = by("floor").sort((a, b) => a.id.localeCompare(b.id));
    const fslots =
      floors.length === 1
        ? [[21, 24, 8]]
        : [
            [21, 26, 7],
            [36, 22, 8],
            [50, 20, 8],
          ];
    floors.forEach((p, i) => {
      const [x, y, r] = fslots[i] ?? [33 + i * 8, 20, 8];
      put(p, x, y, r);
    });
    const toms = by("tom").sort((a, b) => a.id.localeCompare(b.id));
    toms.forEach((p, i) => {
      const t = toms.length === 1 ? 0.5 : i / (toms.length - 1);
      put(p, -7 + 21 * t, 35 + 3 * Math.sin(Math.PI * t), 5 + 1.5 * t);
    });
    by("snare").forEach((p) => put(p, -7, 21, 7));
    by("hat").forEach((p) => put(p, -19, 23, 7));
    const cslot: Record<string, number[]> = { crash1: [-17, 38, 9], crash2: [11, 48, 9], splash: [-3, 47, 5], china: [46, 39, 9] };
    by("crash").forEach((p, i) => {
      const [x, y, r] = cslot[p.id] ?? [-30 + i * 14, 52, 8];
      put(p, x, y, r);
    });
    by("ride").forEach((p) => put(p, 28, 40, 10));
    // 打楽器のパッド: キットの下に横 1 列(多いときは 2 列)。間隔 step は名前の札がぶつからない幅(fit で決める)
    const pads = by("perc");
    const perRow = Math.max(1, Math.min(pads.length, 8));
    pads.forEach((p, i) => {
      const row = Math.floor(i / perRow),
        n = Math.min(perRow, pads.length - row * perRow),
        col = i % perRow;
      out.push({ ...p, x: 12 * IN + (col - (n - 1) / 2) * step, y: (2 + row * 7) * IN, r: 3 * IN });
    });
    return out;
  },
  kitBounds(L: any[]) {
    const ext = (p: any) => [p.rect ? p.rect[0] : p.r, p.rect ? p.rect[1] : p.r];
    return { x0: Math.min(...L.map((p) => p.x - ext(p)[0])), x1: Math.max(...L.map((p) => p.x + ext(p)[0])), y0: Math.min(...L.map((p) => p.y - ext(p)[1])), y1: Math.max(...L.map((p) => p.y + ext(p)[1])) };
  },
  kitLabel(p: Part) {
    return (SHORT()[p.id] ?? p.name) + (p.keys.length > 1 ? `·${p.keys.length}` : "");
  },
  /** 拡大の大きさと置く位置: パーツが占める範囲を、縦横の比を変えずに枠いっぱい(端に 16 px 以上あける)まで拡大する。
   *  打楽器のパッドの間隔は、その拡大で名前の札がぶつからない幅(拡大と間隔を交互に 2 回決め直す) */
  fit(d: Data, w: number, h: number) {
    const key = `${d.instrument}|${w}x${h}`;
    if (SFZ._fit?.key === key) return SFZ._fit;
    const g = (SFZ._mctx ??= document.createElement("canvas").getContext("2d")) as CanvasRenderingContext2D;
    const pad = Math.max(16, Math.round(Math.min(w, h) * 0.06));
    let step = 64,
      out: any = null;
    for (let it = 0; it < 2; it++) {
      out = SFZ.fitWith(d, w, h, step, pad, g);
      g.font = `${out.sc < 0.32 ? 9 : 10}px sans-serif`;
      const label = Math.max(0, ...SFZ.pieces(d).filter((p: Part) => p.kind === "perc").map((p: Part) => g.measureText(SFZ.kitLabel(p)).width + 6)) + 14;
      step = Math.max(64, label / Math.max(out.sc, 1e-3));
    }
    const f = { ...out, key, refit: 0 };
    // 絵の大きさが決まる前(幅・高さが 0)は覚えない
    if (w > 20 && h > 20) SFZ._fit = f;
    return f;
  },
  fitWith(d: Data, w: number, h: number, step: number, pad: number, g: CanvasRenderingContext2D) {
    const L = SFZ.layout(d, step);
    const B = SFZ.kitBounds(L),
      bw = Math.max(1, B.x1 - B.x0),
      bh = Math.max(1, B.y1 - B.y0);
    const span = (sc: number) => {
      g.font = `${sc < 0.32 ? 9 : 10}px sans-serif`;
      let l = Infinity,
        r = -Infinity;
      L.forEach((p: any) => {
        const half = Math.max(p.rect ? p.rect[0] * sc : p.r * sc, (g.measureText(SFZ.kitLabel(p)).width + 6) / 2);
        l = Math.min(l, p.x * sc - half);
        r = Math.max(r, p.x * sc + half);
      });
      return [l, r];
    };
    let sc = Math.min((w - pad * 2) / bw, (h - pad * 2 - 16) / bh);
    let [l, r] = span(sc);
    if (r - l > w - pad * 2) {
      sc *= (w - pad * 2) / (r - l);
      [l, r] = span(sc);
    }
    const ox = (w - (r - l)) / 2 - l;
    const oy = (h - Math.max(1, bh * sc)) / 2 - B.y0 * sc;
    return { sc: Math.max(sc, 1e-3), ox, oy, step };
  },
  /** 今の拡大での並び */
  laid(d: Data, w: number, h: number) {
    return SFZ.layout(d, SFZ.fit(d, w, h).step);
  },
  geo(d: Data, p: any, w: number, h: number) {
    const { sc, ox, oy } = SFZ.fit(d, w, h);
    const rx = (p.rect ? p.rect[0] : p.r) * sc,
      ry = (p.rect ? p.rect[1] : p.r) * sc;
    return { x: ox + p.x * sc, y: oy + p.y * sc, rx, ry, rect: !!p.rect, sc };
  },
  /** 鍵盤の図の並び(割り当てのある鍵盤の範囲を、オクターブの端まで広げる) */
  keyRange(d: Data): [number, number] {
    const ks = SFZ.info(d)?.keys.map((k: any) => k.key) ?? [60];
    const lo = Math.min(...ks),
      hi = Math.max(...ks);
    return [Math.max(0, lo - (lo % 12)), Math.min(127, hi + (11 - (hi % 12)))];
  },
  pianoKeys(d: Data, w: number, h: number) {
    const [lo, hi] = SFZ.keyRange(d),
      isB = (m: number) => [1, 3, 6, 8, 10].includes(m % 12);
    const whites: number[] = [];
    for (let m = lo; m <= hi; m++) if (!isB(m)) whites.push(m);
    const ww = w / Math.max(1, whites.length),
      keys: { m: number; black: boolean; x: number; w: number; h: number }[] = [];
    whites.forEach((m, i) => keys.push({ m, black: false, x: i * ww, w: ww, h }));
    for (let m = lo; m <= hi; m++)
      if (isB(m)) {
        const i = whites.indexOf(m - 1);
        keys.push({ m, black: true, x: (i + 1) * ww - ww * 0.32, w: ww * 0.64, h: h * 0.6 });
      }
    return keys;
  },
  /** クリックした所の鍵盤(物の鍵盤の並び)。上に描いた物(シンバル・パッド)を先に見る */
  mapHit(d: Data, x: number, y: number, w: number, h: number): number[] | null {
    if (!SFZ.isKit(d)) {
      const top = 6,
        kh = h - top - 6;
      const K = SFZ.pianoKeys(d, w, kh);
      const mapped = new Set(SFZ.info(d)?.keys.map((k: any) => k.key) ?? []);
      const b = K.filter((k: any) => k.black).find((k: any) => x >= k.x && x <= k.x + k.w && y >= top && y <= top + k.h);
      const k = b ?? K.find((k: any) => !k.black && x >= k.x && x < k.x + k.w && y >= top);
      return k && mapped.has(k.m) ? [k.m] : null;
    }
    const lb = (SFZ._labelHits ?? []).find((q: any) => x >= q.box[0] && x <= q.box[2] && y >= q.box[1] && y <= q.box[3]);
    const L = SFZ.laid(d, w, h);
    if (lb) return L.find((p: any) => p.id === lb.id)?.keys.map((k: any) => k.key) ?? null;
    for (const p of [...L].reverse()) {
      const G = SFZ.geo(d, p, w, h);
      if (G.rect ? Math.abs(x - G.x) <= G.rx && Math.abs(y - G.y) <= G.ry : Math.hypot((x - G.x) / G.rx, (y - G.y) / G.ry) <= 1) return p.keys.map((k: any) => k.key);
    }
    return null;
  },
  drawKit(d: Data, g: CanvasRenderingContext2D, w: number, h: number, key: number | null) {
    const L = SFZ.laid(d, w, h),
      selPart = key != null ? SFZ.partOf(d, key) : null;
    L.forEach((p: any) => {
      const G = SFZ.geo(d, p, w, h),
        on = p.keys.some((k: any) => d.playing === k.key),
        sel = p.id === selPart?.id,
        hov = SFZ.hover && p.keys.some((k: any) => SFZ.hover.includes(k.key));
      const cym = ["hat", "crash", "ride"].includes(p.kind);
      g.save();
      const outline = (pad: number) => {
        g.beginPath();
        if (G.rect) g.roundRect(G.x - G.rx - pad, G.y - G.ry - pad, (G.rx + pad) * 2, (G.ry + pad) * 2, 6);
        else g.ellipse(G.x, G.y, G.rx + pad, G.ry + pad, 0, 0, Math.PI * 2);
      };
      if (p.kind === "perc") {
        outline(0);
        g.fillStyle = on ? "rgba(37,189,177,0.6)" : "#2a2f36";
        g.fill();
        g.strokeStyle = "#5a6470";
        g.lineWidth = 1.5;
        g.stroke();
      } else if (p.kind === "kick") {
        outline(0);
        g.fillStyle = on ? "rgba(37,189,177,0.5)" : "#3a3530";
        g.fill();
        g.strokeStyle = "#7a5a3a";
        g.lineWidth = 3;
        g.stroke();
        g.strokeStyle = "rgba(255,255,255,0.12)";
        g.lineWidth = 1;
        [-0.5, 0, 0.5].forEach((f) => {
          g.beginPath();
          g.moveTo(G.x + f * G.rx, G.y - G.ry);
          g.lineTo(G.x + f * G.rx, G.y + G.ry);
          g.stroke();
        });
      } else if (cym) {
        outline(0);
        g.fillStyle = on ? "rgba(37,189,177,0.55)" : p.id === "china" ? "rgba(214,150,90,0.38)" : "rgba(232,196,106,0.32)";
        g.fill();
        g.strokeStyle = "rgba(232,196,106,0.8)";
        g.lineWidth = 1.2;
        g.stroke();
        g.strokeStyle = "rgba(232,196,106,0.35)";
        [0.7, 0.45].forEach((f) => {
          g.beginPath();
          g.ellipse(G.x, G.y, G.rx * f, G.ry * f, 0, 0, Math.PI * 2);
          g.stroke();
        });
        g.fillStyle = "rgba(232,196,106,0.6)";
        g.beginPath();
        g.ellipse(G.x, G.y, G.rx * 0.16, G.ry * 0.16, 0, 0, Math.PI * 2);
        g.fill();
      } else {
        outline(0);
        g.fillStyle = "#2b2b2b";
        g.fill();
        g.strokeStyle = "#6a6a6a";
        g.lineWidth = 3;
        g.stroke();
        g.beginPath();
        g.ellipse(G.x, G.y, G.rx * 0.84, G.ry * 0.84, 0, 0, Math.PI * 2);
        g.fillStyle = on ? "rgba(37,189,177,0.55)" : "rgba(216,210,196,0.22)";
        g.fill();
      }
      if (hov) {
        g.strokeStyle = css("--human");
        g.lineWidth = 2;
        outline(3);
        g.stroke();
      }
      if (sel) {
        g.strokeStyle = "#fff";
        g.lineWidth = 2.5;
        outline(5);
        g.stroke();
      }
      if (p.keys.some((k: any) => SFZ.edited(d, k.key))) {
        g.fillStyle = css("--human");
        g.beginPath();
        g.arc(G.x + G.rx * 0.7, G.y - G.ry * 0.7, 4, 0, Math.PI * 2);
        g.fill();
      }
      g.restore();
    });
    // 名前: 全部のパーツの上に、黒い下地つきで重ねる。名前どうしがぶつかる所は少し上下にずらす
    const fs = SFZ.geo(d, { x: 0, y: 0, r: 1 }, w, h).rx < 0.32 ? 9 : 10;
    g.font = `${fs}px sans-serif`;
    const placed: number[][] = [];
    const hits: { id: string; box: number[] }[] = [];
    const hit = (a: number[], b: number[]) => a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3];
    [...L]
      .sort((a: any, b: any) => +(b.id === selPart?.id) - +(a.id === selPart?.id))
      .forEach((p: any) => {
        const G = p.lab ? SFZ.geo(d, { x: p.lab[0], y: p.lab[1], r: 0 }, w, h) : SFZ.geo(d, p, w, h),
          name = SFZ.kitLabel(p),
          tw = g.measureText(name).width;
        let box: number[] | null = null;
        const sx = tw / 2 + 6,
          tries = [
            [0, 0],
            [0, 13],
            [0, -13],
            [0, 24],
            [0, -24],
            [-sx, 0],
            [sx, 0],
            [-sx, 13],
            [sx, 13],
            [-sx, -13],
            [sx, -13],
            [0, 34],
            [0, -34],
          ];
        const over = (bx: number[]) => placed.reduce((a, q) => a + Math.max(0, Math.min(bx[2], q[2]) - Math.max(bx[0], q[0])) * Math.max(0, Math.min(bx[3], q[3]) - Math.max(bx[1], q[1])), 0);
        let best: [number, number[]] | null = null;
        for (const [dx, dy] of tries) {
          const cy = G.y + dy,
            x0 = clamp(G.x + dx - tw / 2 - 3, 0, w - tw - 6),
            bx = [x0, cy - 7, x0 + tw + 6, cy + 6];
          if (bx[1] < 0 || bx[3] > h) continue;
          if (!placed.some((q) => hit(bx, q))) {
            box = bx;
            break;
          }
          const o = over(bx);
          if (!best || o < best[0]) best = [o, bx];
        }
        if (!box) {
          const x0 = clamp(G.x - tw / 2 - 3, 0, w - tw - 6);
          box = best ? best[1] : [x0, G.y - 7, x0 + tw + 6, G.y + 6];
        }
        placed.push(box);
        hits.push({ id: p.id, box });
        g.fillStyle = "rgba(0,0,0,0.72)";
        g.beginPath();
        g.roundRect(box[0], box[1], box[2] - box[0], box[3] - box[1], 3);
        g.fill();
        g.fillStyle = p.id === selPart?.id ? "#fff" : css("--text");
        g.fillText(name, box[0] + 3, box[3] - 3);
      });
    SFZ._labelHits = hits;
    // 名前の札を置いた後の端で、上下・左右の余白をそろえ直す(数回まで)
    const F = SFZ.fit(d, w, h);
    if (F.refit < 3) {
      const bs = L.map((p: any) => {
        const G = SFZ.geo(d, p, w, h);
        return [G.x - G.rx, G.y - G.ry, G.x + G.rx, G.y + G.ry];
      });
      placed.forEach((b) => bs.push(b));
      const left = Math.min(...bs.map((a: number[]) => a[0])),
        right = w - Math.max(...bs.map((a: number[]) => a[2])),
        top = Math.min(...bs.map((a: number[]) => a[1])),
        bottom = h - Math.max(...bs.map((a: number[]) => a[3]));
      F.refit++;
      if (Math.abs(top - bottom) > 2 || Math.abs(left - right) > 2) {
        F.oy += (bottom - top) / 2;
        F.ox += (right - left) / 2;
        g.clearRect(0, 0, w, h);
        return SFZ.drawKit(d, g, w, h, key);
      }
    }
  },
  drawPiano(d: Data, g: CanvasRenderingContext2D, w: number, h: number, key: number | null) {
    const top = 6,
      kh = h - top - 6;
    const K = SFZ.pianoKeys(d, w, kh);
    const mapped = new Set(SFZ.info(d)?.keys.map((k: any) => k.key) ?? []);
    K.forEach((k: any) => {
      const has = mapped.has(k.m);
      g.fillStyle = k.black ? (has ? "#2f4a48" : "#161616") : has ? "#cfe9e6" : "#6a6a66";
      if (d.playing === k.m) g.fillStyle = "#25bdb1";
      g.fillRect(k.x + 0.5, top, k.w - 1, k.h - (k.black ? 0 : 1));
      g.strokeStyle = "#111";
      g.lineWidth = 1;
      g.strokeRect(k.x + 0.5, top + 0.5, k.w - 1, k.h - 1);
      if (SFZ.edited(d, k.m)) {
        g.fillStyle = css("--human");
        g.beginPath();
        g.arc(k.x + k.w / 2, top + k.h - 10, 3, 0, Math.PI * 2);
        g.fill();
      }
      if (key === k.m) {
        g.strokeStyle = "#fff";
        g.lineWidth = 2;
        g.strokeRect(k.x + 1.5, top + 1.5, k.w - 3, k.h - 3);
      }
      if (SFZ.hover?.includes(k.m)) {
        g.strokeStyle = css("--human");
        g.lineWidth = 1.5;
        g.strokeRect(k.x + 1, top + 1, k.w - 2, k.h - 2);
      }
      if (!k.black && k.m % 12 === 0 && k.w >= 14) {
        g.font = "9px sans-serif";
        g.textAlign = "center";
        g.fillStyle = "#222";
        g.fillText(noteName(k.m), k.x + k.w / 2, top + kh - 3);
        g.textAlign = "left";
      }
    });
  },
  draw() {
    const d = shown();
    if (!d) return;
    const info = SFZ.info(d);
    const c = byId<HTMLCanvasElement>("sfzKit");
    if (!c || !info) return;
    const { g, w, h } = ctx2d(c);
    const key: number | null = SFZ.selKey(d);
    const kit = SFZ.isKit(d);
    if (kit) SFZ.drawKit(d, g, w, h, key);
    else SFZ.drawPiano(d, g, w, h, key);
    const ki = byId("sfzInfo");
    if (ki) {
      const hk: number[] | null = SFZ.hover;
      if (kit) {
        const p = hk ? SFZ.partOf(d, hk[0]) : null;
        ki.textContent = p
          ? tr(
              `${p.name} · ${p.keys.map((k: any) => `${k.art ? k.art + " " : ""}${noteName(k.key)}`).join("・")}${p.keys.length > 1 ? `(叩き分け ${p.keys.length} つ。下の「叩く所」で選ぶ)` : ""} · クリックで叩いて選ぶ`,
              `${p.name} · ${p.keys.map((k: any) => `${k.art ? k.art + " " : ""}${noteName(k.key)}`).join(", ")}${p.keys.length > 1 ? ` (${p.keys.length} hit spots; pick one with “Hit spot” below)` : ""} · click to hit and select`,
            )
          : tr(`上から見た並び(下が叩く人の側)· ${SFZ.pieces(d).length} 個のパーツ · 「·3」は叩き分けの数 · クリックで叩いて選ぶ`, `Seen from above (drummer at the bottom) · ${SFZ.pieces(d).length} pieces · “·3” = hit spots · click to hit and select`);
      } else {
        const k = hk ? SFZ.keyInfo(d, hk[0]) : null;
        ki.textContent = k
          ? tr(`${noteName(k.key)} · 強さの段 ${k.layers.length} つ · クリックで鳴らして選ぶ`, `${noteName(k.key)} · ${k.layers.length} strength layers · click to play and select`)
          : tr(`明るい鍵盤 = 録音のある鍵盤(${info.keys.length} 音)· クリックで鳴らして選ぶ`, `Light keys have recordings (${info.keys.length} keys) · click to play and select`);
      }
    }
    if (key == null) return;
    // 下: 音の形(叩く強さで鳴る録音)。調整した形を重ねる(薄い色 = 調整する前)
    const P: Piece = SFZ.pieceOf(d, key),
      ev = SFZ.hitOf(d, key);
    const kinfo = SFZ.keyInfo(d, key);
    const layers: api.SfzLayer[] = kinfo?.layers ?? [];
    const li = layers.findIndex((l) => ev >= l.vel_lo && ev <= l.vel_hi);
    // 強さの効き方(エンジンと同じ形: (1 − t) + t × 強さ²)
    const gainAt = (v: number, vt: number) => 1 - vt / 100 + (vt / 100) * Math.pow(v / 127, 2);
    const sc = byId<HTMLCanvasElement>("sfzSample");
    if (sc) {
      const { g: g2, w: w2, h: h2 } = ctx2d(sc);
      const wv = SFZ.wave(d, key, ev);
      const wi = byId("sfzWaveInfo");
      if (!wv) {
        g2.fillStyle = css("--faint");
        g2.font = "11px sans-serif";
        g2.fillText(tr("録音を読み込んでいます…", "Loading the recording…"), 8, h2 / 2);
        if (wi) wi.textContent = "";
      } else {
        g2.save();
        g2.translate(4, 0);
        drawAudio(g2, wv.peaks, w2 - 8, 6, h2 - 30, "rgba(127,211,204,0.25)");
        const gain = Math.pow(10, P.vol / 20) * (gainAt(ev, P.vt) / Math.max(gainAt(ev, 100), 1e-3)),
          n = wv.peaks.length / 2,
          cut = P.len / 100,
          tail = cut * 0.25;
        // 長さ: 切る所の手前(長さの 1/4)で消えていく。切る所より後は鳴らない
        const y2 = wv.peaks.map((v: number, i: number) => {
          const t = Math.floor(i / 2) / n;
          const f = t >= cut ? 0 : t > cut - tail ? Math.pow(1e-5, (t - (cut - tail)) / tail) : 1;
          return clamp(v * gain * f, -1, 1);
        });
        drawAudio(g2, y2, w2 - 8, 6, h2 - 30, "rgb(37,189,177)");
        g2.restore();
        timeRuler(g2, 4, w2 - 4, h2 - 15, wv.seconds, tr(" 秒", " s"));
        const rr = li >= 0 ? layers[li].rr : 1;
        if (wi)
          wi.textContent = tr(
            `叩く強さ ${ev} で鳴る録音(強さで切り替わる ${layers.length} つのうち ${li + 1} つ目${rr > 1 ? `。続けて叩くと ${rr} つを順番に使う` : ""})· ${wv.seconds.toFixed(2)} 秒 · ${wv.sample.split("/").pop()}${SFZ.edited(d, key) ? " · 薄い色 = 調整する前" : ""} · クリックで鳴らす`,
            `Recording played at strength ${ev} (layer ${li + 1} of ${layers.length}${rr > 1 ? `; repeated hits cycle through ${rr}` : ""}) · ${wv.seconds.toFixed(2)} s · ${wv.sample.split("/").pop()}${SFZ.edited(d, key) ? " · faint = before adjusting" : ""} · click to play`,
          );
      }
    }
    // 強さの効き方(横 = 叩く強さ 1〜127、縦 = 音量 0〜100 %)。点線 = SFZ のまま
    const vc = byId<HTMLCanvasElement>("sfzVelCurve");
    if (vc) {
      const { g: g4, w: w4, h: h4 } = ctx2d(vc);
      const x0 = 30,
        x1 = w4 - 6,
        y0 = 6,
        y1 = h4 - 14,
        xs = (v: number) => x0 + (v / 127) * (x1 - x0),
        ys = (a: number) => y1 - a * (y1 - y0);
      g4.strokeStyle = "#262626";
      g4.fillStyle = css("--faint");
      g4.font = "9px sans-serif";
      [0, 0.5, 1].forEach((a) => {
        g4.beginPath();
        g4.moveTo(x0, ys(a));
        g4.lineTo(x1, ys(a));
        g4.stroke();
        g4.fillText(`${Math.round(a * 100)}%`, 2, ys(a) + 3);
      });
      [1, 64, 127].forEach((v) => g4.fillText(String(v), Math.min(xs(v) - 4, w4 - 18), h4 - 3));
      const line = (vt: number) => {
        g4.beginPath();
        for (let v = 1; v <= 127; v += 2) (v === 1 ? g4.moveTo : g4.lineTo).call(g4, xs(v), ys(gainAt(v, vt)));
      };
      g4.setLineDash([3, 3]);
      g4.strokeStyle = "rgba(230,230,230,0.45)";
      line(100);
      g4.stroke();
      g4.setLineDash([]);
      g4.strokeStyle = css("--accent");
      g4.lineWidth = 2;
      line(P.vt);
      g4.stroke();
      g4.lineWidth = 1;
      g4.fillStyle = "#fff";
      g4.beginPath();
      g4.arc(xs(ev), ys(gainAt(ev, P.vt)), 3.5, 0, Math.PI * 2);
      g4.fill();
      const hv = SFZ.hoverVel;
      if (hv != null) {
        g4.strokeStyle = css("--human");
        g4.beginPath();
        g4.moveTo(xs(hv), y0);
        g4.lineTo(xs(hv), y1);
        g4.stroke();
        g4.fillStyle = css("--human");
        g4.beginPath();
        g4.arc(xs(hv), ys(gainAt(hv, P.vt)), 3.5, 0, Math.PI * 2);
        g4.fill();
      }
      const vi = byId("sfzVelInfo");
      if (vi)
        vi.textContent =
          hv != null
            ? tr(`叩く強さ ${hv} → 音量 ${Math.round(gainAt(hv, P.vt) * 100)} %(SFZ のままなら ${Math.round(gainAt(hv, 100) * 100)} %)`, `Strength ${hv} → level ${Math.round(gainAt(hv, P.vt) * 100)} % (as in the SFZ: ${Math.round(gainAt(hv, 100) * 100)} %)`)
            : tr(`強さの効き方: 叩く強さ ${ev} → 音量 ${Math.round(gainAt(ev, P.vt) * 100)} %(白い点。点線 = SFZ のまま)`, `Strength ${ev} → level ${Math.round(gainAt(ev, P.vt) * 100)} % (white dot; dotted = as in the SFZ)`);
    }
    // 強さの帯(横 = 強さ 1〜127)。白い線 = 叩く強さ
    const lc = byId<HTMLCanvasElement>("sfzLayers");
    if (lc) {
      const { g: g3, w: w3, h: h3 } = ctx2d(lc);
      const xs = (v: number) => 6 + (v / 127) * (w3 - 12);
      layers.forEach((l, i) => {
        const on = ev >= l.vel_lo && ev <= l.vel_hi;
        g3.fillStyle = on ? "rgba(37,189,177,0.6)" : "rgba(37,189,177,0.18)";
        g3.fillRect(xs(l.vel_lo - 1) + 1, 4, xs(l.vel_hi) - xs(l.vel_lo - 1) - 2, h3 - 20);
        g3.fillStyle = css("--text");
        g3.font = "10px sans-serif";
        const lab = layers.length === 1 ? "1" : i === 0 ? tr("1 弱", "1 soft") : i === layers.length - 1 ? tr(`${i + 1} 強`, `${i + 1} hard`) : String(i + 1);
        if (xs(l.vel_hi) - xs(l.vel_lo - 1) > 22) g3.fillText(lab, xs(l.vel_lo - 1) + 4, 16);
      });
      const vx = xs(ev);
      g3.fillStyle = "#fff";
      g3.fillRect(vx - 1, 2, 2, h3 - 16);
      g3.fillStyle = css("--faint");
      g3.font = "9px sans-serif";
      [1, 64, 127].forEach((v) => g3.fillText(String(v), Math.min(xs(v) - 4, w3 - 18), h3 - 3));
    }
    ui.panes.forEach((p) => p.draw());
  },
  /** 鍵盤で叩いたとき: 割り当てのある鍵盤なら光らせる(選択は変えない) */
  onKey(m: number, on: boolean) {
    const d = cur();
    const has = SFZ.info(d)?.keys.some((k: any) => k.key === m);
    d.playing = on && has ? m : null;
    SFZ.draw();
  },
  reload() {
    SFZ._fit = null;
  },
  refresh() {
    rerender();
  },
  help: (): [string, string][] => [
    [tr("ドラムキット", "Drum kit"), tr("SFZ の割り当て(鍵盤の番号)から自動で並べる。クリックで叩いて選ぶ(白く囲まれ、下で調整できる)。「·3」は叩き分けの数。ドラムでない SFZ は鍵盤の図", "Laid out automatically from the SFZ key mapping. Click to hit and select (outlined in white; adjust it below). “·3” is the number of hit spots. Non-drum SFZs show a keyboard")],
    [tr("叩く所", "Hit spot"), tr("叩き分けのあるパーツ(スネアの真ん中・ふち・クロススティック、ハイハットの閉・ペダル・開 など)は、下の見出しの「叩く所」で選ぶ", "For pieces with several hit spots (snare center / rim / cross-stick, hi-hat closed / pedal / open…), pick one with “Hit spot” in the heading below")],
    [tr("キット全体の調整", "Whole-kit controls"), tr("この SFZ が用意しているつまみ(スナップ・マイクの量など)。全部に効く。選んでいる鍵盤に効くつまみは水色", "Controls the SFZ provides (snap, mic levels…). They affect everything; the ones that affect the selected key are teal")],
    [tr("◯◯を調整", "Adjust …"), tr("選んだ鍵盤だけの音量・音程・長さ・強さの効き方。調整した鍵盤には絵に青い点が付く", "Level, pitch, length and velocity response for the selected key only. Adjusted keys get a blue dot in the picture")],
    [tr("叩く強さ", "Hit strength"), tr("選んだ鍵盤を絵や鍵盤で叩くときの強さ。鍵盤ごとに別の値。帯で、その強さで鳴る録音が分かる(クリックでその強さにして叩く)。曲で鳴る音は曲のノートの強さのまま", "Strength used when hitting the selected key from the picture or keys; separate per key. The bar shows which recording plays at that strength (click to hit at that strength). Notes in the song keep their own velocity")],
    [tr("音の形", "Sound shape"), tr("選んだ鍵盤の録音の波形(叩く強さで鳴るもの)。調整した形を重ねる(薄い色 = 調整する前)", "The waveform of the selected key's recording at the hit strength, with the adjusted shape on top (faint = before adjusting)")],
    [tr("使っている SFZ", "SFZ in use"), tr("SFZ のフォルダ(全部の曲で共通)にある SFZ を使う。「SFZ を替える…」でこのトラックの音源を替える", "Uses an SFZ from the SFZ folder (shared by every song). “Change SFZ…” switches this track's instrument")],
  ],
};

register(SFZ);
export default SFZ;
