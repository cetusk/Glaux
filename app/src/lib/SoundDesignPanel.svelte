<script lang="ts">
  // インスペクター(音作り): トラック 1 本(またはマスター)の音源・エフェクト・送り・つなぎを扱う右のパネル。
  // 見出しに名前・種類・M/S を固定し、中身は折りたたみ式のセクション(畳むと要約を出す)。
  // 開いている間、タイムラインとピアノロールはこの幅だけ押し縮められる(App)。幅は左端をドラッグで変える。
  // すべての編集は Command API(apply_edit)経由なので履歴に載り undo できる。
  import { showError } from "./toast.svelte";
  import { plural, tr } from "./i18n.svelte";
  import { paramDesc, paramLabel } from "./paramText";
  import * as api from "./api";
  import Icon from "./Icon.svelte";
  import { flip } from "svelte/animate";
  import { newClipId, newFxId } from "./ids";
  import { deviceIcon, deviceKind, deviceName } from "./instruments";
  import { fmtValue, fromPos, SLIDER_MAX, toPos } from "./params";
  import {
    effectiveLinks,
    insertBeforeOutput,
    IR_FROM_FILE,
    irChoices,
    sampleChoices,
    moveIndexFor,
    processingOrder,
    serialLinks,
    serialOrder,
    trackChoices,
    unlinkBridging,
    fxKindDesc,
    fxKindName,
  } from "./fx";
  import { loadIrFromFile } from "./ir";
  import { keepInView } from "./menu";
  import { PHRASE_LEN, PHRASE_NAME, phraseNotes } from "./phrase";
  import {
    inspectorStore,
    instrumentPickerStore,
    MASTER_FOCUS_ID,
    projectRev,
    saveInspectorWidth,
    soundDesignStore,
    soundEditorStore,
    viewStore,
  } from "./selection.svelte";
  import { editorKindOf } from "./soundEditor/core.svelte";
  import "./soundEditor/editors";
  import type { EffectView, ParamView, Project, Track, TrackParams } from "./types";

  let { project }: { project: Project } = $props();

  const track = $derived.by((): Track | null => {
    const focus = soundDesignStore.focus;
    if (!focus) return null;
    return project.tracks.find((t) => t.id === focus.trackId) ?? null;
  });

  // ---- CLAP プラグインのプリセット ----
  let clapPresetList = $state<api.ClapPreset[] | null>(null);
  let clapCategories = $state<{ name: string; count: number }[]>([]);
  let clapPresetTrack = $state<string | null>(null);
  let clapPresetBusy = $state(false);
  let clapPresetMsg = $state<string | null>(null);
  let clapCategory = $state("");
  let clapSearch = $state("");
  async function loadClapPresets(trackId: string, rescan = false) {
    clapPresetBusy = true;
    clapPresetMsg = null;
    try {
      const r = await api.clapPresets(trackId, rescan);
      clapPresetList = r.presets;
      clapCategories = r.categories;
      clapPresetTrack = trackId;
    } catch (e) {
      clapPresetList = [];
      clapPresetMsg = String(e);
    } finally {
      clapPresetBusy = false;
    }
  }
  $effect(() => {
    const t = track;
    if (t?.device?.type === "clap" && clapPresetTrack !== t.id && !clapPresetBusy) {
      clapCategory = "";
      clapSearch = "";
      loadClapPresets(t.id);
    }
  });
  const shownClapPresets = $derived.by(() => {
    const q = clapSearch.trim().toLowerCase();
    return (clapPresetList ?? []).filter(
      (p) =>
        (!clapCategory || p.category === clapCategory) &&
        (!q || p.name.toLowerCase().includes(q) || p.category.toLowerCase().includes(q)),
    );
  });
  const currentClapPreset = $derived(
    typeof track?.device?.params?.preset === "string" ? (track.device.params.preset as string) : null,
  );
  async function applyClapPreset(p: api.ClapPreset) {
    if (!track || clapPresetBusy) return;
    clapPresetBusy = true;
    clapPresetMsg = tr(`「${p.name}」を読み込み中…`, `Loading "${p.name}"…`);
    try {
      await api.clapLoadPreset(track.id, p.id);
      clapPresetMsg = null;
    } catch (e) {
      clapPresetMsg = String(e);
    } finally {
      clapPresetBusy = false;
    }
  }

  /// マスターバスを開いている(エフェクトチェーンだけを扱う)
  const isMaster = $derived(soundDesignStore.focus?.trackId === MASTER_FOCUS_ID);
  /// 履歴ラベル用の対象名
  const targetName = $derived(isMaster ? tr("マスター", "Master") : (track?.name ?? ""));

  // トラックが消えたら閉じる
  $effect(() => {
    if (soundDesignStore.focus && !track && !isMaster) {
      soundDesignStore.focus = null;
    }
  });

  function close() {
    soundDesignStore.focus = null;
  }

  // ---- spec + 現在値の取得(プロジェクトが変わるたびに再取得 = AI の編集も反映) ----

  let info = $state<TrackParams | null>(null);
  let loadError = $state<string | null>(null);

  $effect(() => {
    const t = track;
    const master = isMaster;
    void projectRev.value; // 依存: どの編集でも現在値を取り直す
    if (!t && !master) {
      info = null;
      return;
    }
    // 取り直している間に次の編集・トラックの切り替えがあったら、前の要求の応答は捨てる(古い値で上書きしない)
    let stale = false;
    (master ? api.getMasterParams() : api.getTrackParams(t!.id))
      .then((r) => {
        if (stale) return;
        info = r;
        loadError = null;
      })
      .catch((e) => {
        if (!stale) loadError = String(e);
      });
    return () => {
      stale = true;
    };
  });

  function setLegato(name: "glide_ms" | "legato_ms", value: number) {
    const t = track;
    if (!t) return;
    const what = name === "glide_ms" ? tr("ポルタメントの滑る時間", "glide time") : tr("レガートのつなぎ目", "legato overlap");
    applyEdit(
      [{ op: "set_param", track: t.id, path: `track/${name}`, value }],
      tr(`${t.name} の${what}を ${value}ms に`, `Set ${t.name} ${what} to ${value}ms`),
    );
  }

  function resetLegato() {
    const t = track;
    if (!t) return;
    const cmds = (["glide_ms", "legato_ms"] as const)
      .filter((n) => t[n] !== undefined)
      .map((n) => ({ op: "unset_param", track: t.id, path: `track/${n}` }));
    if (cmds.length > 0) applyEdit(cmds, tr(`${t.name} のつなぎ方を既定に戻す`, `Reset ${t.name} legato settings`));
  }

  async function applyEdit(commands: unknown[], label: string) {
    try {
      await api.applyEdit(commands, label);
    } catch (e) {
      loadError = String(e);
    }
  }

  // ---- パラメータ編集 ----

  /** つまみを値にするコマンド(対象が無ければ null) */
  function paramCommand(p: ParamView, raw: string | number | boolean): unknown | null {
    const t = track;
    if (!t && !isMaster) return null;
    let value: unknown = raw;
    if (p.range.kind === "float") value = Number(raw);
    if (p.range.kind === "int") value = Math.round(Number(raw));
    if (p.range.kind === "bool") value = Boolean(raw);
    return isMaster
      ? { op: "set_master_param", path: p.path, value }
      : { op: "set_param", track: t!.id, path: p.path, value };
  }

  /** つまみの表示名・説明(CLAP のつまみはプラグインの名前のまま)。kind は音源・エフェクトの種類 */
  function pLabel(p: ParamView, kind: string | null): string {
    return kind === "clap" ? p.display_name : paramLabel(kind, p.name, p.display_name);
  }
  function pDesc(p: ParamView, kind: string | null): string {
    return kind === "clap" ? p.description : paramDesc(kind, p.name, p.description);
  }

  /// 音源のつまみの種類(内蔵は音源名、それ以外は device の type)
  const instKind = $derived.by((): string => {
    const d = track?.device;
    if (!d) return "subtractive";
    return !d.type || d.type === "builtin" ? (d.name ?? "subtractive") : d.type;
  });

  function commitParam(p: ParamView, raw: string | number | boolean, kind: string | null = null) {
    const cmd = paramCommand(p, raw);
    if (cmd) applyEdit([cmd], tr(`${targetName} の ${p.display_name} を変更`, `Change ${targetName} ${pLabel(p, kind)}`));
  }

  /// ドラッグ中の値(パラメータのパス → 値)。離すまで表示だけ変える
  let dragValues = $state<Record<string, number>>({});

  function onSliderInput(p: ParamView, e: Event) {
    const v = fromPos(p, Number((e.currentTarget as HTMLInputElement).value));
    dragValues[p.path] = v;
    // ドラッグ中から音に反映する(履歴には載せない。離したときに 1 回だけ確定する)
    const cmd = paramCommand(p, v);
    if (cmd) api.previewEdit([cmd]);
  }

  function onSliderChange(p: ParamView, e: Event, kind: string | null = null) {
    const v = fromPos(p, Number((e.currentTarget as HTMLInputElement).value));
    delete dragValues[p.path];
    commitParam(p, v, kind);
  }

  // ---- つまみのグループ(名前で分ける。知らない名前は「その他」) ----
  const GROUPS: [string, string, string[]][] = [
    ["音の元", "Source", ["waveform", "unison", "detune", "sub", "noise", "table", "position", "ratio", "feedback", "pick", "root", "tune", "algorithm", "sample", "partials"]],
    ["オペレーター", "Operators", ["op1_ratio", "op1_level", "op1_attack", "op1_decay", "op1_sustain", "op2_ratio", "op2_level", "op2_attack", "op2_decay", "op2_sustain", "op3_ratio", "op3_level", "op3_attack", "op3_decay", "op3_sustain", "op4_ratio", "op4_level", "op4_attack", "op4_decay", "op4_sustain"]],
    ["粒", "Grains", ["grain_ms", "density", "spray_ms", "pitch_rand", "window", "scan"]],
    ["倍音", "Harmonics", ["tilt", "odd_even", "formant_hz", "formant_db", "formant_width", "damping", "inharmonic"]],
    ["再生(ループ・スライス)", "Playback (loop/slice)", ["loop", "loop_start", "loop_end", "loop_xfade_ms", "slices", "orig_bpm", "stereo"]],
    ["広がり・揺らぎ", "Width & drift", ["spread", "analog", "wobble"]],
    ["変調", "Modulation", ["index", "index_decay", "index_sustain", "pos_env", "pos_decay", "lfo_rate", "lfo_depth"]],
    ["音色", "Tone", ["filter_type", "cutoff", "resonance", "drive", "filter_env", "vel_cutoff", "key_track", "tone", "brightness", "vel_bright"]],
    ["フィルタのエンベロープ", "Filter envelope", ["filter_attack", "filter_decay", "filter_sustain"]],
    ["LFO", "LFO", ["lfo1_target", "lfo1_shape", "lfo1_rate", "lfo1_depth", "lfo2_target", "lfo2_shape", "lfo2_rate", "lfo2_depth"]],
    ["エンベロープ", "Envelope", ["attack", "decay", "sustain", "release", "attack_ms", "decay_ms", "release_ms"]],
    ["奏法の効き", "Articulations", ["staccato", "accent", "vibrato", "bend", "legato", "portamento", "palm_mute"]],
    ["出力", "Output", ["gain_db"]],
  ];

  function grouped(params: ParamView[]): { name: string; params: ParamView[] }[] {
    const out = GROUPS.map(([ja, en, keys]) => ({ name: tr(ja, en), params: params.filter((p) => keys.includes(p.name)) }));
    const known = new Set(GROUPS.flatMap(([, , keys]) => keys));
    out.push({ name: tr("その他", "Other"), params: params.filter((p) => !known.has(p.name)) });
    return out.filter((g) => g.params.length > 0);
  }

  // ---- セクションの開閉(覚えておく) ----
  function loadClosed(): Record<string, boolean> {
    try {
      return JSON.parse(localStorage.getItem("glaux.inspector.closed") ?? "{}");
    } catch {
      return {};
    }
  }
  let closed = $state<Record<string, boolean>>(loadClosed());
  function toggleSec(key: string) {
    closed[key] = !closed[key];
    try {
      localStorage.setItem("glaux.inspector.closed", JSON.stringify(closed));
    } catch {
      // 保存できなくても動作には関係しない
    }
  }

  // ---- 幅の変更(左端をドラッグ) ----
  function startResize(e: PointerEvent) {
    e.preventDefault();
    const startX = e.clientX;
    const startW = inspectorStore.width;
    const move = (ev: PointerEvent) => {
      inspectorStore.width = Math.round(Math.min(640, Math.max(300, startW + (startX - ev.clientX))));
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      saveInspectorWidth();
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  // ---- 見出しの M / S ----
  function toggleProp(prop: "mute" | "solo") {
    const t = track;
    if (!t) return;
    // 聴き方なので履歴に積まない
    api.setListen([{ op: "set_track_prop", id: t.id, prop, value: !t[prop] }]).catch(() => {});
  }

  // ---- 音源 ----
  function openPicker(e: MouseEvent, tab?: "preset") {
    const t = track;
    if (!t) return;
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    instrumentPickerStore.open = { trackId: t.id, x: Math.max(8, r.right - 560), y: r.bottom + 4, tab };
  }

  let presetName = $state("");
  let savingPreset = $state(false);

  async function savePreset() {
    const t = track;
    const name = presetName.trim();
    if (!t || !name) return;
    try {
      await api.savePreset(t.id, name);
      presetName = "";
      savingPreset = false;
    } catch (e) {
      showError(tr("プリセットを保存できませんでした", "Couldn't save the preset"), e);
    }
  }

  // ---- センド / バス ----

  const buses = $derived(project.tracks.filter((t) => t.kind === "bus"));
  /// バスを開いているとき: 送ってきているトラック
  const busSenders = $derived.by(() => {
    const t = track;
    if (!t || t.kind !== "bus") return [];
    return project.tracks.flatMap((src) =>
      (src.sends ?? [])
        .filter((s) => s.target === t.id)
        .map((s) => ({ name: src.name, level_db: s.level_db, pre: s.pre_fader ?? false })),
    );
  });

  function setSend(bus: Track, levelDb: number, preFader: boolean) {
    const t = track;
    if (!t) return;
    applyEdit(
      [{ op: "set_send", track: t.id, target: bus.id, level_db: levelDb, pre_fader: preFader }],
      tr(
        `${t.name} から ${bus.name} への送りを ${levelDb.toFixed(1)} dB に`,
        `Set send from ${t.name} to ${bus.name} to ${levelDb.toFixed(1)} dB`,
      ),
    );
  }

  function removeSend(bus: Track) {
    const t = track;
    if (!t) return;
    applyEdit(
      [{ op: "set_send", track: t.id, target: bus.id }],
      tr(`${t.name} から ${bus.name} への送りを外す`, `Remove send from ${t.name} to ${bus.name}`),
    );
  }

  /// 送る量のドラッグ中の値(バス ID → dB)。離すまで表示だけ変える
  let sendDrag = $state<Record<string, number>>({});

  const sendSummary = $derived.by(() => {
    const t = track;
    if (!t || buses.length === 0) return tr("バスなし", "No buses");
    const on = buses.filter((b) => t.sends?.some((s) => s.target === b.id));
    return on.length === 0 ? tr("送っていない", "Not sending") : on.map((b) => b.name).join(tr("・", ", "));
  });

  // ---- エフェクト ----

  /// インストール済みの CLAP エフェクト(インスペクターを開いたときに一度読む)
  let clapEffects = $state<api.ClapPluginInfo[]>([]);
  let clapEffectsLoaded = false;
  $effect(() => {
    if (soundDesignStore.focus && !clapEffectsLoaded) {
      clapEffectsLoaded = true;
      api
        .clapPlugins()
        .then((r) => (clapEffects = r.plugins.filter((p) => p.effect)))
        .catch(() => {});
    }
  });

  /// エフェクトの表示名(CLAP はプラグイン名)
  function fxLabel(fx: EffectView): string {
    if (fx.label) return fx.label;
    if (fx.name !== "clap") return fx.name;
    return fx.plugin_name ?? fx.plugin_id ?? "CLAP";
  }

  function openFxGui(fx: EffectView) {
    fxMenu = null;
    api.clapOpenGui(null, fx.id).catch((e) => showError(tr("プラグインの画面を開けませんでした", "Couldn't open the plugin window"), e));
  }

  /// `name` は内蔵エフェクト名か "clap:<plugin_id>"(CLAP プラグイン)
  function addEffect(name: string) {
    addMenu = null;
    const t = track;
    if ((!t && !isMaster) || !name) return;
    const clapId = name.startsWith("clap:") ? name.slice(5) : null;
    const effect = clapId
      ? { id: newFxId(), type: "clap", plugin_id: clapId }
      : { id: newFxId(), type: "builtin", name };
    const label = clapId ? (clapEffects.find((p) => p.id === clapId)?.name ?? clapId) : name;
    applyEdit(
      [isMaster ? { op: "add_master_effect", effect } : { op: "add_effect", track: t!.id, effect }],
      tr(`${targetName} に ${label} を追加`, `Add ${label} to ${targetName}`),
    );
  }

  function removeEffect(fx: EffectView) {
    fxMenu = null;
    applyEdit(
      [{ op: "remove_effect", id: fx.id }],
      tr(`${targetName} の ${fxLabel(fx)} を削除`, `Remove ${fxLabel(fx)} from ${targetName}`),
    );
  }

  function toggleBypass(fx: EffectView) {
    applyEdit(
      [{ op: "set_effect_bypass", id: fx.id, bypass: !fx.bypass }],
      tr(
        `${targetName} の ${fxLabel(fx)} を${fx.bypass ? "有効に" : "バイパス"}`,
        `${fx.bypass ? "Enable" : "Bypass"} ${fxLabel(fx)} on ${targetName}`,
      ),
    );
  }

  /// 畳んでいるエフェクト(ID)
  let fxFolded = $state<Record<string, boolean>>({});

  /// エフェクトの ⋯ メニューと、追加のメニュー
  let fxMenu = $state<{ fx: EffectView; x: number; y: number } | null>(null);
  let addMenu = $state<{ x: number; y: number } | null>(null);

  function menuAt(e: MouseEvent) {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    return { x: r.left, y: r.bottom + 4 };
  }

  // CLAP エフェクトのプリセット(開いているエフェクト 1 つ分)
  let fxPresetOpen = $state<string | null>(null);
  let fxPresetList = $state<api.ClapPreset[] | null>(null);
  let fxPresetMsg = $state<string | null>(null);
  let fxPresetBusy = $state(false);
  async function toggleFxPresets(fx: EffectView) {
    fxMenu = null;
    if (fxPresetOpen === fx.id) {
      fxPresetOpen = null;
      return;
    }
    fxPresetOpen = fx.id;
    fxFolded[fx.id] = false;
    fxPresetList = null;
    fxPresetMsg = null;
    try {
      const r = await api.clapPresets(null, false, fx.id);
      fxPresetList = r.presets;
      fxPresetMsg = r.current_preset ? tr(`今: ${r.current_preset}`, `Current: ${r.current_preset}`) : null;
    } catch (e) {
      fxPresetList = [];
      fxPresetMsg = String(e);
    }
  }
  /// カテゴリごとにまとめた一覧(select の optgroup 用)
  const fxPresetGroups = $derived.by(() => {
    const groups = new Map<string, api.ClapPreset[]>();
    for (const p of (fxPresetList ?? []).slice(0, 1000)) {
      const g = groups.get(p.category) ?? [];
      g.push(p);
      groups.set(p.category, g);
    }
    return [...groups.entries()];
  });
  async function loadFxPreset(fx: EffectView, presetId: string) {
    if (!presetId || fxPresetBusy) return;
    fxPresetBusy = true;
    fxPresetMsg = tr("読み込み中…", "Loading…");
    try {
      const r = await api.clapLoadPreset(null, presetId, fx.id);
      fxPresetMsg = tr(`今: ${r.preset}(Ctrl+Z で戻せます)`, `Current: ${r.preset} (Ctrl+Z to undo)`);
    } catch (e) {
      fxPresetMsg = String(e);
    } finally {
      fxPresetBusy = false;
    }
  }

  // ---- エフェクトの並べ替え(カードの左端をつかんで上下にドラッグ) ----
  // つかんだカードはポインターについて動き、ほかはすき間を空けるように滑る。離すと並びを先に画面へ反映し
  // (保存と再取得を待たない)、animate:flip で収まる所へ滑り込む
  let fxList = $state<HTMLElement | undefined>(undefined);
  let fxDrag = $state<{ id: string; from: number; to: number; dy: number; h: number } | null>(null);
  /// 先に反映している並び(エフェクト ID)。実際の並びが追いついたら外す
  let fxOrder = $state<string[] | null>(null);
  let fxOrderTimer: ReturnType<typeof setTimeout> | undefined;

  /// エフェクトのつながり(ノード表示の線)。表が無ければ並び順の直列
  const rawLinks = $derived(isMaster ? project.master.fx_links : track?.fx_links);
  const fxLinks = $derived(effectiveLinks(info?.effects ?? [], rawLinks));
  /// 鳴るエフェクトの処理の順
  const soundingIds = $derived(processingOrder(info?.effects ?? [], fxLinks));
  /// ただの 1 本の直列なら並べ替えられる(分岐・合流があるときはノード表示で)
  const serial = $derived(serialOrder(fxLinks) !== null);
  /// 鳴らない(入力から出口まで線でたどれない)エフェクト。並びとは別に畳んで出す
  const parkedEffects = $derived((info?.effects ?? []).filter((f) => !soundingIds.includes(f.id)));

  const shownEffects = $derived.by((): EffectView[] => {
    const byAll = new Map((info?.effects ?? []).map((f) => [f.id, f]));
    const list = soundingIds.map((id) => byAll.get(id)).filter((f): f is EffectView => !!f);
    const order = fxOrder;
    if (!order) return list;
    const byId = new Map(list.map((f) => [f.id, f]));
    const out = order.map((id) => byId.get(id)).filter((f): f is EffectView => !!f);
    return out.length === list.length ? out : list;
  });

  $effect(() => {
    const order = fxOrder;
    if (order && soundingIds.join() === order.join()) fxOrder = null;
  });

  function onFxGripDown(e: PointerEvent, fx: EffectView, index: number) {
    if (e.button !== 0 || !fxList || !serial) return;
    e.preventDefault();
    const cards = [...fxList.querySelectorAll<HTMLElement>(".fx")];
    const rects = cards.map((c) => c.getBoundingClientRect());
    const self = rects[index];
    if (!self) return;
    const startY = e.clientY;
    const gap = rects.length > 1 ? rects[1].top - rects[0].bottom : 6;
    fxDrag = { id: fx.id, from: index, to: index, dy: 0, h: self.height + gap };
    const move = (ev: PointerEvent) => {
      if (!fxDrag) return;
      const dy = ev.clientY - startY;
      const center = self.top + self.height / 2 + dy;
      const to = rects.filter((r, i) => i !== index && r.top + r.height / 2 < center).length;
      fxDrag = { ...fxDrag, dy, to };
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      const d = fxDrag;
      fxDrag = null;
      if (!d) return;
      if (d.to === d.from) {
        if (d.dy !== 0)
          cards[index]?.animate([{ transform: `translateY(${d.dy}px)` }, { transform: "none" }], {
            duration: 160,
            easing: "ease-out",
          });
        return;
      }
      const ids = shownEffects.map((f) => f.id);
      ids.splice(d.from, 1);
      ids.splice(d.to, 0, d.id);
      // 表が無ければ並び順を動かす(外してあるものも含めた全体の位置に直す)。表があれば直列の線を引き直す
      let cmd: unknown;
      if (rawLinks) {
        cmd = isMaster ? { op: "set_fx_links", links: serialLinks(ids) } : { op: "set_fx_links", track: track?.id, links: serialLinks(ids) };
      } else {
        const toIndex = moveIndexFor(info?.effects ?? [], d.id, d.to);
        if (toIndex === null) return;
        cmd = { op: "move_effect", id: d.id, to_index: toIndex };
      }
      fxOrder = ids;
      clearTimeout(fxOrderTimer);
      fxOrderTimer = setTimeout(() => (fxOrder = null), 3000);
      api
        .applyEdit(
          [cmd],
          tr(`${targetName} の ${fxLabel(fx)} を ${d.to + 1} 番目へ`, `Move ${fxLabel(fx)} on ${targetName} to position ${d.to + 1}`),
        )
        .catch((e) => {
          fxOrder = null;
          loadError = String(e);
        });
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  /// 鳴らないエフェクトを線に戻す(出口の前 / 表が無ければ並びの最後)
  function unpark(fx: EffectView) {
    if (rawLinks) {
      const next = insertBeforeOutput(unlinkBridging(fxLinks, fx.id), fx.id);
      applyEdit(
        [isMaster ? { op: "set_fx_links", links: next } : { op: "set_fx_links", track: track?.id, links: next }],
        tr(`${targetName} の ${fxLabel(fx)} を出口の前につなぐ`, `Connect ${fxLabel(fx)} on ${targetName} before the output`),
      );
      return;
    }
    const all = info?.effects ?? [];
    const to = moveIndexFor(all, fx.id, all.filter((f) => !f.parked).length);
    const cmds: unknown[] = [
      { op: "set_effect_prop", id: fx.id, prop: "parked", value: false },
      { op: "set_effect_prop", id: fx.id, prop: "pos", value: null },
    ];
    if (to !== null) cmds.push({ op: "move_effect", id: fx.id, to_index: to });
    applyEdit(cmds, tr(`${targetName} の ${fxLabel(fx)} を線に戻す`, `Reconnect ${fxLabel(fx)} on ${targetName}`));
  }

  /// ミキサーのノード表示で開く(並べ替え・外す・名前やメモ)
  function openInMixer() {
    const id = soundDesignStore.focus?.trackId;
    if (!id) return;
    viewStore.mixerTrack = id;
    viewStore.highlightFx = null;
    viewStore.main = "mixer";
  }

  /// ドラッグ中の各カードのずれ
  function fxShift(i: number): number {
    const d = fxDrag;
    if (!d) return 0;
    if (i === d.from) return d.dy;
    if (d.from < d.to && i > d.from && i <= d.to) return -d.h;
    if (d.to < d.from && i >= d.to && i < d.from) return d.h;
    return 0;
  }

  // ---- 試聴フレーズ ----

  const phraseClip = $derived(track?.clips.find((c) => c.kind === "midi" && c.name === PHRASE_NAME) ?? null);
  let srcMenu = $state<{ x: number; y: number } | null>(null);

  function insertPhrase() {
    srcMenu = null;
    const t = track;
    if (!t || phraseClip) return;
    // 既存クリップの後ろに置く(なければ先頭)
    const start = t.clips.reduce((end, c) => Math.max(end, c.start + c.length), 0);
    applyEdit(
      [
        {
          op: "add_clip",
          track: t.id,
          clip: { id: newClipId(), name: PHRASE_NAME, start, length: PHRASE_LEN, kind: "midi", notes: phraseNotes() },
        },
      ],
      tr(`${t.name} に試聴フレーズを挿入`, `Insert audition phrase into ${t.name}`),
    );
  }

  function removePhrase() {
    srcMenu = null;
    const t = track;
    const c = phraseClip;
    if (!t || !c) return;
    applyEdit([{ op: "remove_clip", id: c.id }], tr(`${t.name} の試聴フレーズを削除`, `Remove audition phrase from ${t.name}`));
  }

  function closeMenus() {
    fxMenu = null;
    addMenu = null;
    srcMenu = null;
  }

  const KIND_LABEL = { midi: ["MIDI", "MIDI"], audio: ["音声", "Audio"], bus: ["バス", "Bus"] } as const;
  const kindLabel = (k: keyof typeof KIND_LABEL) => tr(KIND_LABEL[k][0], KIND_LABEL[k][1]);
</script>

{#snippet paramCell(p: ParamView, kind: string | null)}
  <div class="pm" title={pDesc(p, kind)}>
    <div class="pm-top">
      <span class="pm-name">{pLabel(p, kind)}</span>
      {#if p.range.kind === "float" || p.range.kind === "int"}<span class="pm-val">{fmtValue(p, dragValues[p.path])}</span>{/if}
    </div>
    {#if p.range.kind === "float" || p.range.kind === "int"}
      <input
        type="range"
        min="0"
        max={SLIDER_MAX}
        step="1"
        value={toPos(p, Number(p.current))}
        oninput={(e) => onSliderInput(p, e)}
        onchange={(e) => onSliderChange(p, e, kind)}
        aria-label={pLabel(p, kind)}
      />
    {:else if p.range.kind === "bool"}
      <label class="pm-bool"
        ><input type="checkbox" checked={Boolean(p.current)} onchange={(e) => commitParam(p, (e.currentTarget as HTMLInputElement).checked, kind)} />
        {p.current ? tr("オン", "On") : tr("オフ", "Off")}</label
      >
    {:else}
      {@const tc = trackChoices(p, project.tracks) ?? irChoices(p, project.assets) ?? sampleChoices(p, project.assets)}
      <select
        value={String(p.current)}
        onchange={(e) => {
          const el = e.currentTarget as HTMLSelectElement;
          if (el.value === IR_FROM_FILE) {
            el.value = String(p.current);
            loadIrFromFile(isMaster ? null : (track?.id ?? null), p.path);
          } else commitParam(p, el.value, kind);
        }}
        aria-label={pLabel(p, kind)}
      >
        {#if tc}
          {#each tc as c (c.value)}<option value={c.value}>{c.label}</option>{/each}
        {:else}
          {#if !p.range.choices.includes(String(p.current))}
            <!-- 選択肢に無い値(import_wavetable で音声から作ったテーブル = 素材の ID) -->
            <option value={String(p.current)}>{p.name === "table" && String(p.current).startsWith("sha256:") ? tr("音声から作ったテーブル", "Table from audio") : String(p.current)}</option>
          {/if}
          {#each p.range.choices as c (c)}
            <option value={c}>{c}</option>
          {/each}
        {/if}
      </select>
    {/if}
  </div>
{/snippet}

{#snippet secHead(key: string, title: string, summary: string)}
  <button class="sec-h" onclick={() => toggleSec(key)} aria-expanded={!closed[key]}>
    <span class="chev" class:shut={closed[key]}><Icon name="chevron-down" size={14} /></span>
    <span class="sec-t">{title}</span>
    <span class="sec-sum">{closed[key] ? summary : ""}</span>
  </button>
{/snippet}

{#if track || isMaster}
  <aside class="sd-panel" style="width:{inspectorStore.width}px" aria-label={tr("インスペクター", "Inspector")}>
    <div class="resize" role="separator" aria-orientation="vertical" title={tr("ドラッグで幅を変える", "Drag to resize")} onpointerdown={startResize}></div>
    <div class="sd-head">
      {#if isMaster}
        <Icon name="sliders-horizontal" />
        <b class="sd-title">{tr("マスター", "Master")}</b>
        <span class="sd-kind">{tr("曲全体に掛かるエフェクト", "Effects on the whole song")}</span>
      {:else if track}
        <span class="dot" style={track.color ? `background:${track.color}` : ""}></span>
        <b class="sd-title" title={track.name}>{track.name}</b>
        <span class="sd-kind">{kindLabel(track.kind)}</span>
        <button class="btn letter" class:m-on={track.mute} onclick={() => toggleProp("mute")} title={tr("ミュート", "Mute")} aria-pressed={track.mute}>M</button>
        <button class="btn letter" class:s-on={track.solo} onclick={() => toggleProp("solo")} title={tr("ソロ(このトラックだけ聴く)", "Solo (hear only this track)")} aria-pressed={track.solo}
          >S</button
        >
      {/if}
      <button class="btn sm icon ghost" onclick={close} title={tr("閉じる", "Close")} aria-label={tr("閉じる", "Close")}><Icon name="x" /></button>
    </div>

    {#if loadError}
      <div class="sd-error">{loadError}</div>
    {/if}

    <div class="sd-body">
      {#if track && track.kind === "bus"}
        <section class="sec">
          {@render secHead("bus", tr("バス", "Bus"), tr(`受けている ${busSenders.length} 本`, plural(busSenders.length, "sender")))}
          {#if !closed.bus}
            <div class="sec-b">
              <div class="hint">
                {tr(
                  "各トラックのインスペクターの「送り」で、このバスへ送る量を決めます。ここに挿したエフェクト(リバーブ・ディレイ等)を複数のトラックで共有できます。",
                  "Set how much each track sends here under Sends in its inspector. Effects inserted here (reverb, delay, etc.) are shared by those tracks.",
                )}
              </div>
              {#each busSenders as s (s.name)}
                <div class="kv"><span>{s.name}</span><span>{s.level_db.toFixed(1)} dB{s.pre ? tr("・フェーダー前", " · pre-fader") : ""}</span></div>
              {:else}
                <div class="hint">{tr("まだどのトラックからも送られていません。", "No tracks send to this bus yet.")}</div>
              {/each}
            </div>
          {/if}
        </section>
      {/if}

      {#if track && track.kind === "midi"}
        <!-- 音源 -->
        <section class="sec">
          {@render secHead("src", tr("音源", "Instrument"), deviceName(track.device))}
          {#if !closed.src}
            <div class="sec-b">
              <div class="src-card">
                <span class="src-ic"><Icon name={deviceIcon(track.device)} size={22} /></span>
                <span class="src-nm"
                  ><b>{track.device?.type === "clap" ? (info?.device.name ?? deviceName(track.device)) : deviceName(track.device)}</b><small
                    >{deviceKind(track.device)}{info?.device.is_default_fallback ? tr("(未設定なので既定の音)", " (not set; default sound)") : ""}</small
                  ></span
                >
                <button class="btn sm" onclick={(e) => openPicker(e)} title={tr(
                    "音源を変える(内蔵・音色のプリセット・SoundFont・CLAP・サンプル)",
                    "Change the instrument (built-in, sound presets, SoundFont, CLAP, sample)",
                  )}
                  >{tr("変更", "Change")}<Icon name="chevron-down" /></button
                >
              </div>
              {#if editorKindOf(track)}
                <button
                  class="btn sm primary"
                  class:on={soundEditorStore.trackId === track.id}
                  onclick={() => (soundEditorStore.trackId = soundEditorStore.trackId === track!.id ? null : track!.id)}
                  title={tr(
                    "広いパネルで、絵を見ながら音色を作り込む(波形・倍音・音量の変わり方など)",
                    "Shape the sound in a wide panel while looking at pictures (waveform, harmonics, envelope…)",
                  )}><Icon name="sliders-horizontal" />{soundEditorStore.trackId === track.id ? tr("音色エディタを閉じる", "Close the sound editor") : tr("音色を作り込む", "Open the sound editor")}</button
                >
              {/if}
              <div class="row">
                <button class="btn sm" onclick={(e) => openPicker(e, "preset")} title={tr(
                    "保存した音色のプリセット(音源 + エフェクト一式。全プロジェクト共通)から選ぶ",
                    "Choose a saved sound preset (instrument + effects; shared by all projects)",
                  )}
                  ><Icon name="save" />{tr("音色のプリセット", "Sound presets")}</button
                >
                {#if track.device?.type !== "clap"}
                  <button class="btn sm" class:on={savingPreset} onclick={() => (savingPreset = !savingPreset)} title={tr("今の音(音源とエフェクト)を音色のプリセットとして保存", "Save the current sound (instrument and effects) as a sound preset")}
                    ><Icon name="plus" />{tr("保存", "Save")}</button
                  >
                {:else}
                  <button class="btn sm" onclick={() => api.clapOpenGui(track!.id).catch((e) => showError(tr("プラグインの画面を開けませんでした", "Couldn't open the plugin window"), e))}
                    ><Icon name="app-window" />{tr("プラグインの画面", "Plugin window")}</button
                  >
                {/if}
                <span class="grow"></span>
                <button class="btn sm icon ghost" onclick={(e) => (srcMenu = menuAt(e))} title={tr("その他(試聴フレーズなど)", "More (audition phrase, etc.)")} aria-label={tr("その他", "More")}
                  ><Icon name="ellipsis" /></button
                >
              </div>
              {#if savingPreset}
                <div class="row">
                  <!-- svelte-ignore a11y_autofocus -->
                  <input
                    class="grow"
                    type="text"
                    placeholder={tr("プリセットの名前", "Preset name")}
                    autofocus
                    bind:value={presetName}
                    onkeydown={(e) => {
                      if (e.key === "Enter" && !e.isComposing) {
                        e.preventDefault();
                        savePreset();
                      } else if (e.key === "Escape") savingPreset = false;
                    }}
                  />
                  <button class="btn sm primary" disabled={!presetName.trim()} onclick={savePreset}>{tr("保存", "Save")}</button>
                </div>
              {/if}

              {#if track.device?.type === "clap"}
                <!-- CLAP: 音作りはプラグイン自身の画面で。ここではプリセットを選ぶ -->
                <div class="grp-t">
                  {tr("プリセット", "Presets")}{currentClapPreset ? tr(`(今: ${currentClapPreset})`, ` (current: ${currentClapPreset})`) : ""}
                  <button class="btn sm icon ghost" disabled={clapPresetBusy} title={tr("プリセットを探し直す(追加した後など)", "Rescan presets (e.g. after adding some)")} aria-label={tr("探し直す", "Rescan")} onclick={() => track && loadClapPresets(track.id, true)}
                    ><Icon name="refresh-cw" /></button
                  >
                  <button
                    class="btn sm icon ghost"
                    title={tr(
                      "プラグインの今の設定をプロジェクトに保存する(画面を閉じたときや操作の後にも自動で保存されます)",
                      "Save the plugin's current settings to the project (also saved automatically when its window closes or after edits)",
                    )}
                    aria-label={tr("設定を保存", "Save settings")}
                    onclick={() => api.clapSaveState(track!.id).catch((e) => showError(tr("プラグインの状態を保存できませんでした", "Couldn't save the plugin state"), e))}
                    ><Icon name="save" /></button
                  >
                </div>
                {#if clapPresetList === null}
                  <div class="hint">{tr("プリセットを探しています…", "Looking for presets…")}</div>
                {:else if clapPresetList.length === 0}
                  <div class="hint">
                    {tr(
                      "このプラグインはプリセットを Glaux に公開していません。プラグインの画面のプリセットメニューから選んでください(選んだ設定は自動でプロジェクトに保存され、Ctrl+Z で戻せます)。",
                      "This plugin doesn't expose its presets to Glaux. Pick one from the preset menu in the plugin window (the choice is saved to the project automatically and can be undone with Ctrl+Z).",
                    )}
                  </div>
                {:else}
                  <div class="row">
                    <select bind:value={clapCategory} title={tr("カテゴリ(フォルダ)", "Category (folder)")}>
                      <option value="">{tr(`すべて(${clapPresetList.length})`, `All (${clapPresetList.length})`)}</option>
                      {#each clapCategories as c (c.name)}
                        <option value={c.name}>{tr(`${c.name || "(未分類)"}(${c.count})`, `${c.name || "(Uncategorized)"} (${c.count})`)}</option>
                      {/each}
                    </select>
                    <input class="grow" type="search" placeholder={tr("名前で絞り込み", "Filter by name")} bind:value={clapSearch} />
                  </div>
                  <div class="preset-list">
                    {#each shownClapPresets.slice(0, 500) as p (p.id)}
                      <button
                        class="preset-item"
                        class:current={p.name === currentClapPreset}
                        disabled={clapPresetBusy}
                        title={`${p.collection}\n${p.category}${p.creators.length ? "\nby " + p.creators.join(", ") : ""}`}
                        onclick={() => applyClapPreset(p)}
                      >
                        <span class="pi-name">{p.name}</span>
                        <span class="pi-cat">{p.category}</span>
                      </button>
                    {/each}
                    {#if shownClapPresets.length > 500}
                      <div class="hint">
                        {tr(
                          `…ほか ${shownClapPresets.length - 500} 件(絞り込んでください)`,
                          `…and ${shownClapPresets.length - 500} more (narrow the search)`,
                        )}
                      </div>
                    {/if}
                  </div>
                {/if}
                {#if clapPresetMsg}<div class="hint">{clapPresetMsg}</div>{/if}
              {:else if info}
                {#each grouped(info.params) as g (g.name)}
                  <div class="grp-t">{g.name}</div>
                  <div class="params">
                    {#each g.params as p (p.path)}
                      {@render paramCell(p, instKind)}
                    {/each}
                  </div>
                {/each}
              {/if}
            </div>
          {/if}
        </section>
      {/if}

      {#if info}
        <!-- エフェクト(トラック・マスター共通) -->
        <section class="sec">
          {@render secHead("fx", tr("エフェクト", "Effects"), shownEffects.length === 0 ? tr("なし", "None") : shownEffects.map((f) => fxLabel(f)).join(" → "))}
          {#if !closed.fx}
            <div class="sec-b">
              <div class="fx-list" bind:this={fxList}>
                {#each shownEffects as fx, i (fx.id)}
                  <div
                    class="fx"
                    class:off={fx.bypass}
                    class:lifted={fxDrag?.id === fx.id}
                    class:sliding={fxDrag !== null && fxDrag.id !== fx.id}
                    style={fxDrag ? `transform:translateY(${fxShift(i)}px)` : ""}
                    animate:flip={{ duration: 180 }}
                  >
                    <div class="fx-h">
                      <span class="grip" class:off={!serial} role="button" tabindex="-1" aria-label={tr("並べ替え", "Reorder")} title={serial ? tr("つかんで上下にドラッグで並べ替え", "Drag up or down to reorder") : tr("分かれたり合流したりしているので、並べ替えはノード表示で", "The chain splits or merges; reorder it in the node view")} onpointerdown={(e) => onFxGripDown(e, fx, i)}
                        ><Icon name="grip-vertical" size={14} /></span
                      >
                      <button
                        class="btn sm icon"
                        class:on={!fx.bypass}
                        onclick={() => toggleBypass(fx)}
                        title={fx.bypass ? tr("バイパス中(クリックで有効に)", "Bypassed (click to enable)") : tr("有効(クリックでバイパス)", "Enabled (click to bypass)")}
                        aria-label={tr("有効 / バイパス", "Enable / bypass")}
                        aria-pressed={!fx.bypass}><Icon name="power" /></button
                      >
                      <button class="fx-name" onclick={() => (fxFolded[fx.id] = !fxFolded[fx.id])} title={fx.plugin_id ?? tr("クリックで開く / 畳む", "Click to expand / collapse")}>
                        <b>{fxLabel(fx)}</b>{#if fx.name === "clap"}<span class="clap-chip">CLAP</span>{/if}
                      </button>
                      {#if fx.name === "clap" && !fx.missing}
                        <button class="btn sm icon ghost" onclick={() => openFxGui(fx)} title={tr("プラグインの画面を開く", "Open the plugin window")} aria-label={tr("プラグインの画面", "Plugin window")}
                          ><Icon name="app-window" /></button
                        >
                      {/if}
                      <button class="btn sm icon ghost" onclick={() => (fxFolded[fx.id] = !fxFolded[fx.id])} aria-label={fxFolded[fx.id] ? tr("開く", "Expand") : tr("畳む", "Collapse")}
                        ><Icon name={fxFolded[fx.id] ? "chevron-right" : "chevron-down"} /></button
                      >
                      <button class="btn sm icon ghost" onclick={(e) => (fxMenu = { fx, ...menuAt(e) })} title={tr("その他(プリセット・削除)", "More (presets, remove)")} aria-label={tr("その他", "More")}
                        ><Icon name="ellipsis" /></button
                      >
                    </div>
                    {#if !fxFolded[fx.id]}
                      <div class="fx-b">
                        {#if fxPresetOpen === fx.id}
                          <div class="fx-presets">
                            {#if fxPresetList === null}
                              <div class="hint">{tr("プリセットを探しています…", "Looking for presets…")}</div>
                            {:else if fxPresetList.length === 0}
                              <div class="hint">
                                {tr(
                                  "このプラグインはプリセットを Glaux に公開していません(例: Surge XT Effects)。プラグインの画面のプリセットメニューから選んでください。選んだ設定は自動でプロジェクトに保存され、Ctrl+Z で戻せます。",
                                  "This plugin doesn't expose its presets to Glaux (e.g. Surge XT Effects). Pick one from the preset menu in the plugin window. The choice is saved to the project automatically and can be undone with Ctrl+Z.",
                                )}
                              </div>
                            {:else}
                              <select class="grow" disabled={fxPresetBusy} onchange={(e) => loadFxPreset(fx, (e.currentTarget as HTMLSelectElement).value)}>
                                <option value="">{tr(`プリセットを選ぶ…(${fxPresetList.length})`, `Choose a preset… (${fxPresetList.length})`)}</option>
                                {#each fxPresetGroups as [cat, list] (cat)}
                                  <optgroup label={cat || tr("(未分類)", "(Uncategorized)")}>
                                    {#each list as p (p.id)}
                                      <option value={p.id}>{p.name}</option>
                                    {/each}
                                  </optgroup>
                                {/each}
                              </select>
                            {/if}
                            {#if fxPresetMsg}<div class="hint">{fxPresetMsg}</div>{/if}
                          </div>
                        {/if}
                        {#if fx.missing}
                          <div class="hint warn">
                            {tr(
                              "このプラグインはこの PC に見つかりません(音は素通し)。インストールすると元の設定で鳴ります。",
                              "This plugin isn't installed on this PC (audio passes through). Install it to hear it with its saved settings.",
                            )}
                          </div>
                        {/if}
                        <div class="params">
                          {#each fx.params as p (p.path)}
                            {@render paramCell(p, fx.name)}
                          {/each}
                        </div>
                        {#if fx.name === "clap" && (fx.param_total ?? 0) > fx.params.length}
                          <div class="hint">
                            {tr(
                              `ほか ${(fx.param_total ?? 0) - fx.params.length} 個のつまみはプラグインの画面か AI から操作できます。`,
                              `${plural((fx.param_total ?? 0) - fx.params.length, "more parameter")} can be controlled from the plugin window or by the AI.`,
                            )}
                          </div>
                        {/if}
                      </div>
                    {/if}
                  </div>
                {/each}
              </div>
              {#if parkedEffects.length > 0}
                <div class="parked">
                  <div
                    class="parked-h"
                    title={tr(
                      "入力から出口まで線でたどれないエフェクト。設定は残っていて、音は通らない",
                      "Effects not on a path from input to output. Their settings are kept, but no audio passes through",
                    )}
                  >
                    <Icon name="unplug" size={12} />{tr(
                      `鳴らない ${parkedEffects.length}(線がつながっていない)`,
                      `${parkedEffects.length} silent (not connected)`,
                    )}
                  </div>
                  {#each parkedEffects as fx (fx.id)}
                    <div class="parked-row" title={fx.note ?? ""}>
                      <span class="parked-nm">{fxLabel(fx)}{#if fx.note}<small>{fx.note}</small>{/if}</span>
                      <button class="btn sm ghost" onclick={() => unpark(fx)} title={tr("出口の前につなぐ", "Connect before the output")}><Icon name="plug" />{tr("つなぐ", "Connect")}</button>
                      <button class="btn sm icon ghost" onclick={() => removeEffect(fx)} title={tr("削除(Ctrl+Z で戻せます)", "Remove (Ctrl+Z to undo)")} aria-label={tr("削除", "Remove")}><Icon name="trash-2" /></button>
                    </div>
                  {/each}
                </div>
              {/if}
              <div class="row">
                <button class="btn sm add-fx" onclick={(e) => (addMenu = menuAt(e))}><Icon name="plus" />{tr("エフェクトを追加", "Add effect")}<Icon name="chevron-down" /></button>
                <button class="btn sm ghost" onclick={openInMixer} title={tr(
                    "ミキサーのノード表示で開く(線でつなぐ・分ける・混ぜる・名前やメモ)",
                    "Open in the mixer's node view (connect, split, mix, names and notes)",
                  )}
                  ><Icon name="sliders-horizontal" />{tr("ノード表示", "Node view")}</button
                >
              </div>
            </div>
          {/if}
        </section>
      {/if}

      {#if track && track.kind !== "bus"}
        <!-- 送り(バスへ送る量) -->
        <section class="sec">
          {@render secHead("send", tr("送り", "Sends"), sendSummary)}
          {#if !closed.send}
            <div class="sec-b">
              {#each buses as bus (bus.id)}
                {@const snd = track.sends?.find((s) => s.target === bus.id)}
                {@const shown = sendDrag[bus.id] ?? snd?.level_db}
                <div
                  class="send"
                  title={tr(
                    "このトラックの音をバスへ送る量。フェーダー後はトラックの音量に追従します",
                    "How much of this track to send to the bus. Post-fader follows the track volume",
                  )}
                >
                  <span class="send-nm">{bus.name}</span>
                  <input
                    type="range"
                    min="-60"
                    max="6"
                    step="0.5"
                    value={snd?.level_db ?? -60}
                    aria-label={tr(`${bus.name} へ送る量`, `Send level to ${bus.name}`)}
                    oninput={(e) => {
                      const v = Number((e.currentTarget as HTMLInputElement).value);
                      sendDrag[bus.id] = v;
                      if (track)
                        api.previewEdit([
                          { op: "set_send", track: track.id, target: bus.id, level_db: v, pre_fader: snd?.pre_fader ?? false },
                        ]);
                    }}
                    onchange={(e) => {
                      delete sendDrag[bus.id];
                      setSend(bus, Number((e.currentTarget as HTMLInputElement).value), snd?.pre_fader ?? false);
                    }}
                  />
                  <span class="pm-val">{shown !== undefined ? `${shown.toFixed(1)} dB` : tr("送らない", "Off")}</span>
                  <button
                    class="btn sm icon ghost"
                    class:on={snd?.pre_fader}
                    disabled={!snd}
                    onclick={() => snd && setSend(bus, snd.level_db, !(snd.pre_fader ?? false))}
                    title={snd?.pre_fader
                      ? tr("フェーダー前から送っている(クリックでフェーダー後に)", "Sending pre-fader (click for post-fader)")
                      : tr("フェーダー後から送っている(クリックでフェーダー前に)", "Sending post-fader (click for pre-fader)")}
                    aria-label={tr("フェーダー前から送る", "Send pre-fader")}><Icon name="arrow-up" /></button
                  >
                  <button class="btn sm icon ghost" disabled={!snd} onclick={() => removeSend(bus)} title={tr("送らない", "Remove send")} aria-label={tr("送らない", "Remove send")}
                    ><Icon name="x" /></button
                  >
                </div>
              {:else}
                <div class="hint">
                  {tr(
                    "バスがありません。タイムラインの「トラックを追加」からバスを作ると、複数のトラックで同じリバーブを共有できます。",
                    "No buses yet. Create one with Add track in the timeline to share one reverb across several tracks.",
                  )}
                </div>
              {/each}
            </div>
          {/if}
        </section>
      {/if}

      {#if track && track.kind === "midi"}
        <!-- レガート / ポルタメントのつなぎ方(奏法 T / P のノートに効く) -->
        <section class="sec">
          {@render secHead(
            "legato",
            tr("つなぎ", "Legato"),
            tr(
              `滑る時間 ${track.glide_ms ?? 150}ms · つなぎ目 ${track.legato_ms ?? 30}ms${track.glide_ms === undefined && track.legato_ms === undefined ? "(既定)" : ""}`,
              `Glide ${track.glide_ms ?? 150}ms · Overlap ${track.legato_ms ?? 30}ms${track.glide_ms === undefined && track.legato_ms === undefined ? " (default)" : ""}`,
            ),
          )}
          {#if !closed.legato}
            <div class="sec-b">
              <div class="params">
                <div
                  class="pm"
                  title={tr(
                    "ポルタメント(P)のノートが直前の音から滑る時間。ゆったりした弦は 250〜400、速いリードは 50〜80",
                    "How long portamento (P) notes glide from the previous note. Slow strings 250–400, fast leads 50–80",
                  )}
                >
                  <div class="pm-top"><span class="pm-name">{tr("滑る時間", "Glide time")}</span><span class="pm-val">{track.glide_ms ?? 150}ms</span></div>
                  <input type="range" min="10" max="1000" step="10" value={track.glide_ms ?? 150} aria-label={tr("滑る時間", "Glide time")}
                    onchange={(e) => setLegato("glide_ms", Number((e.currentTarget as HTMLInputElement).value))} />
                </div>
                <div
                  class="pm"
                  title={tr(
                    "レガート(T)・ポルタメント(P)で前の音と入れ替わる長さ。長いほどふんわり重なる",
                    "How long legato (T) and portamento (P) notes overlap the previous note. Longer blends more softly",
                  )}
                >
                  <div class="pm-top"><span class="pm-name">{tr("つなぎ目", "Overlap")}</span><span class="pm-val">{track.legato_ms ?? 30}ms</span></div>
                  <input type="range" min="5" max="200" step="5" value={track.legato_ms ?? 30} aria-label={tr("つなぎ目", "Overlap")}
                    onchange={(e) => setLegato("legato_ms", Number((e.currentTarget as HTMLInputElement).value))} />
                </div>
              </div>
              <div class="row">
                <span class="hint grow"
                  >{tr(
                    "ピアノロールでノートに T(レガート)/ P(ポルタメント)を付けたときのつながり方",
                    "How notes marked T (legato) / P (portamento) in the piano roll connect",
                  )}</span
                >
                {#if track.glide_ms !== undefined || track.legato_ms !== undefined}
                  <button class="btn sm" onclick={resetLegato}>{tr("既定に戻す", "Reset")}</button>
                {/if}
              </div>
            </div>
          {/if}
        </section>
      {/if}
    </div>
  </aside>

  {#if fxMenu || addMenu || srcMenu}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div class="menu-backdrop" role="presentation" onclick={closeMenus} oncontextmenu={(e) => { e.preventDefault(); closeMenus(); }}></div>
  {/if}
  {#if fxMenu}
    {@const fx = fxMenu.fx}
    <div class="menu" use:keepInView style="left:{fxMenu.x}px;top:{fxMenu.y}px">
      {#if fx.name === "clap" && !fx.missing}
        <button onclick={() => toggleFxPresets(fx)}><Icon name="save" />{tr("プリセットから選ぶ", "Choose a preset")}</button>
        <button onclick={() => openFxGui(fx)}><Icon name="app-window" />{tr("プラグインの画面を開く", "Open plugin window")}</button>
        <div class="menu-sep"></div>
      {/if}
      <button class="danger" onclick={() => removeEffect(fx)}><Icon name="trash-2" />{tr("削除", "Remove")}<span class="key">{tr("Ctrl+Z で戻せます", "Ctrl+Z to undo")}</span></button>
    </div>
  {/if}
  {#if addMenu && info}
    <div class="menu" use:keepInView style="left:{addMenu.x}px;top:{addMenu.y}px">
      <div class="menu-h">{tr("内蔵", "Built-in")}</div>
      {#each info.available_effects as fx (fx.name)}
        <button class="rich" onclick={() => addEffect(fx.name)}><span>{fxKindName(fx.name) ?? fx.name}<small>{fxKindDesc(fx.name, fx.description)}</small></span></button>
      {/each}
      {#if clapEffects.length > 0}
        <div class="menu-sep"></div>
        <div class="menu-h">{tr("CLAP プラグイン", "CLAP plugins")}</div>
        {#each clapEffects as p (p.id)}
          <button class="rich" onclick={() => addEffect(`clap:${p.id}`)}><Icon name="plug" /><span>{p.name}<small>{p.vendor} {p.version}</small></span></button>
        {/each}
      {/if}
    </div>
  {/if}
  {#if srcMenu && track}
    <div class="menu" use:keepInView style="left:{srcMenu.x}px;top:{srcMenu.y}px">
      {#if phraseClip}
        <button onclick={removePhrase}><Icon name="trash-2" />{tr("試聴フレーズを消す", "Remove audition phrase")}</button>
      {:else}
        <button class="rich" onclick={insertPhrase} title={tr("ロングトーン → 刻み → 分散和音 → オクターブ上の 4 小節", "4 bars: long tone → repeated notes → arpeggio → octave up")}
          ><Icon name="music" /><span
            >{tr("試聴フレーズを入れる", "Insert audition phrase")}<small
              >{tr("末尾に 4 小節。再生とループを回しながら調整する", "4 bars at the end. Tweak while it loops")}</small
            ></span
          ></button
        >
      {/if}
    </div>
  {/if}
{/if}

<style>
  .sd-panel {
    position: absolute;
    top: 0;
    right: 0;
    bottom: 0;
    z-index: 6;
    background: var(--bg-panel);
    border-left: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    font-size: var(--fs-md);
  }

  .resize {
    position: absolute;
    left: -3px;
    top: 0;
    bottom: 0;
    width: 6px;
    cursor: col-resize;
    z-index: 2;
    touch-action: none;
  }

  .resize:hover {
    background: var(--accent-dim);
  }

  .sd-head {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 9px 10px 9px 12px;
    border-bottom: 1px solid var(--border);
    color: var(--text-dim);
  }

  .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--text-faint);
    flex-shrink: 0;
  }

  .sd-title {
    font-size: var(--fs-lg);
    color: var(--text);
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .sd-kind {
    font-size: 10px;
    padding: 1px 5px;
    border-radius: 3px;
    background: var(--bg-raised);
  }

  .letter {
    width: 22px;
    height: 20px;
    padding: 0;
    font-size: 10px;
    font-weight: 700;
    border-radius: var(--r-sm);
  }

  .letter.m-on {
    background: var(--mute);
    border-color: var(--mute);
    color: #1a1a1a;
  }

  .letter.s-on {
    background: var(--solo);
    border-color: var(--solo);
    color: #1a1a1a;
  }

  .sd-error {
    background: var(--danger-bg);
    color: var(--danger-text);
    font-size: var(--fs-xs);
    padding: 6px 12px;
  }

  .sd-body {
    flex: 1;
    overflow-y: auto;
  }

  .sec {
    border-bottom: 1px solid var(--border);
  }

  .sec-h {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    border: 0;
    border-radius: 0;
    background: none;
    padding: 8px 12px;
    color: var(--text);
    text-align: left;
  }

  .sec-h:hover:not(:disabled) {
    background: var(--bg-raised);
    border: 0;
    color: var(--text);
  }

  .chev {
    display: inline-flex;
    color: var(--text-dim);
    transition: transform 0.12s;
  }

  .chev.shut {
    transform: rotate(-90deg);
  }

  .sec-t {
    font-weight: 600;
    font-size: var(--fs-md);
  }

  .sec-sum {
    flex: 1;
    min-width: 0;
    text-align: right;
    font-size: var(--fs-xs);
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .sec-b {
    padding: 2px 12px 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .grow {
    flex: 1;
    min-width: 0;
  }

  input[type="text"],
  input[type="search"],
  select {
    height: 24px;
    background: var(--bg-inset);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    font-size: var(--fs-sm);
    padding: 0 6px;
    min-width: 0;
  }

  input[type="range"] {
    width: 100%;
    margin: 0;
    height: 14px;
    accent-color: var(--accent);
  }

  .src-card {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: var(--r-md);
  }

  .src-ic {
    display: inline-flex;
    color: var(--accent);
  }

  .src-nm {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  .src-nm b {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .src-nm small {
    font-size: var(--fs-xs);
    color: var(--text-dim);
  }

  .grp-t {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: var(--fs-xs);
    color: var(--text-dim);
    margin-top: 2px;
  }

  /* つまみは 2 列(名前と値が上、つまみが下) */
  .params {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 6px 14px;
  }

  .pm {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    font-size: var(--fs-xs);
  }

  .pm-top {
    display: flex;
    justify-content: space-between;
    gap: 4px;
  }

  .pm-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .pm-val {
    font-family: var(--mono);
    color: var(--text-dim);
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }

  .pm-bool {
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--text-dim);
  }

  .kv {
    display: flex;
    justify-content: space-between;
    font-size: var(--fs-sm);
  }

  .kv span:last-child {
    color: var(--text-dim);
  }

  .fx-list {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .fx {
    border: 1px solid var(--border);
    border-radius: var(--r-md);
    background: var(--bg);
  }

  .fx.lifted {
    position: relative;
    z-index: 2;
    border-color: var(--accent-dim);
    box-shadow: 0 8px 20px rgba(0, 0, 0, 0.5);
  }

  .fx.sliding {
    transition: transform 0.16s ease;
  }

  .fx-h {
    display: flex;
    align-items: center;
    gap: 3px;
    height: 32px;
    padding: 0 4px 0 2px;
  }

  .grip {
    display: inline-flex;
    color: var(--text-faint);
    cursor: grab;
    touch-action: none;
  }

  .grip:hover {
    color: var(--text);
  }

  .grip.off {
    cursor: not-allowed;
    opacity: 0.35;
  }

  .fx-name {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 6px;
    border: 0;
    background: none;
    padding: 0 4px;
    text-align: left;
    color: var(--text);
  }

  .fx-name:hover:not(:disabled) {
    border: 0;
    color: var(--text);
  }

  .fx-name b {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .fx.off .fx-name b {
    color: var(--text-faint);
    text-decoration: line-through;
  }

  .clap-chip {
    font-size: 9px;
    padding: 1px 4px;
    border-radius: 3px;
    background: color-mix(in srgb, var(--ai) 20%, transparent);
    color: var(--ai);
    flex-shrink: 0;
  }

  .fx-b {
    padding: 2px 10px 10px 24px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .fx.off .fx-b {
    opacity: 0.55;
  }

  .fx-presets {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .add-fx {
    align-self: flex-start;
  }

  .parked {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 6px 8px;
    border: 1px dashed var(--border-strong);
    border-radius: var(--r-md);
  }

  .parked-h {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: var(--fs-xs);
    color: var(--text-faint);
  }

  .parked-row {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .parked-nm {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    font-size: var(--fs-sm);
    color: var(--text-dim);
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .parked-nm small {
    font-size: 10px;
    color: var(--text-faint);
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .send {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 100px 58px 22px 22px;
    align-items: center;
    gap: 6px;
    font-size: var(--fs-sm);
  }

  .send-nm {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .send .pm-val {
    text-align: right;
    font-size: var(--fs-xs);
  }

  .hint {
    font-size: var(--fs-xs);
    color: var(--text-dim);
    line-height: 1.5;
  }

  .hint.warn {
    color: var(--warn);
  }

  .preset-list {
    max-height: 260px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
  }

  .preset-item {
    display: flex;
    justify-content: space-between;
    gap: 8px;
    border: 0;
    border-radius: 0;
    background: none;
    padding: 4px 8px;
    font-size: var(--fs-sm);
    text-align: left;
  }

  .preset-item:hover:not(:disabled) {
    background: var(--bg-raised);
    border: 0;
  }

  .preset-item.current {
    color: var(--accent);
  }

  .pi-cat {
    font-size: var(--fs-xs);
    color: var(--text-dim);
  }

  .menu-backdrop {
    position: fixed;
    inset: 0;
    z-index: 19;
  }

  .menu {
    position: fixed;
    z-index: 20;
    max-height: calc(100vh - 16px);
    overflow-y: auto;
    min-width: 200px;
    max-width: 360px;
    background: var(--bg-panel);
    border: 1px solid var(--border-strong);
    border-radius: var(--r-lg);
    box-shadow: var(--shadow-pop);
    padding: 4px;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .menu button {
    display: flex;
    align-items: center;
    gap: 10px;
    border: 0;
    background: none;
    text-align: left;
    padding: 6px 10px;
    border-radius: var(--r-sm);
    font-size: var(--fs-md);
    --icon-size: 15px;
  }

  .menu button:hover:not(:disabled) {
    background: color-mix(in srgb, var(--accent) 15%, transparent);
  }

  .menu button > :global(.icon) {
    color: var(--text-dim);
  }

  .menu button.rich span {
    display: flex;
    flex-direction: column;
  }

  .menu small {
    font-size: var(--fs-xs);
    color: var(--text-dim);
  }

  .menu .danger {
    color: var(--danger-text);
  }

  .menu .key {
    margin-left: auto;
    padding-left: 20px;
    font-size: var(--fs-xs);
    color: var(--text-faint);
  }

  .menu-h {
    font-size: var(--fs-xs);
    color: var(--text-faint);
    padding: 4px 10px 2px;
  }

  .menu-sep {
    height: 1px;
    background: var(--border);
    margin: 3px 4px;
  }
</style>
