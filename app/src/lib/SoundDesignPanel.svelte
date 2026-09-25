<script lang="ts">
  // インスペクター(音作り): トラック 1 本(またはマスター)の音源・エフェクト・送り・つなぎを扱う右のパネル。
  // 見出しに名前・種類・M/S を固定し、中身は折りたたみ式のセクション(畳むと要約を出す)。
  // 開いている間、タイムラインとピアノロールはこの幅だけ押し縮められる(App)。幅は左端をドラッグで変える。
  // すべての編集は Command API(apply_edit)経由なので履歴に載り undo できる。
  import { showError } from "./toast.svelte";
  import * as api from "./api";
  import Icon from "./Icon.svelte";
  import { flip } from "svelte/animate";
  import { newClipId, newFxId } from "./ids";
  import { deviceIcon, deviceKind, deviceName } from "./instruments";
  import { keepInView } from "./menu";
  import { PHRASE_LEN, PHRASE_NAME, phraseNotes } from "./phrase";
  import {
    inspectorStore,
    instrumentPickerStore,
    MASTER_FOCUS_ID,
    saveInspectorWidth,
    soundDesignStore,
  } from "./selection.svelte";
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
    clapPresetMsg = `「${p.name}」を読み込み中…`;
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
  const targetName = $derived(isMaster ? "マスター" : (track?.name ?? ""));

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
    void project; // 依存: どの編集でも現在値を取り直す
    if (!t && !master) {
      info = null;
      return;
    }
    (master ? api.getMasterParams() : api.getTrackParams(t!.id))
      .then((r) => {
        info = r;
        loadError = null;
      })
      .catch((e) => (loadError = String(e)));
  });

  function setLegato(name: "glide_ms" | "legato_ms", value: number) {
    const t = track;
    if (!t) return;
    const what = name === "glide_ms" ? "ポルタメントの滑る時間" : "レガートのつなぎ目";
    applyEdit([{ op: "set_param", track: t.id, path: `track/${name}`, value }], `${t.name} の${what}を ${value}ms に`);
  }

  function resetLegato() {
    const t = track;
    if (!t) return;
    const cmds = (["glide_ms", "legato_ms"] as const)
      .filter((n) => t[n] !== undefined)
      .map((n) => ({ op: "unset_param", track: t.id, path: `track/${n}` }));
    if (cmds.length > 0) applyEdit(cmds, `${t.name} のつなぎ方を既定に戻す`);
  }

  async function applyEdit(commands: unknown[], label: string) {
    try {
      await api.applyEdit(commands, label);
    } catch (e) {
      loadError = String(e);
    }
  }

  // ---- パラメータ編集 ----

  function commitParam(p: ParamView, raw: string | number | boolean) {
    const t = track;
    if (!t && !isMaster) return;
    let value: unknown = raw;
    if (p.range.kind === "float") value = Number(raw);
    if (p.range.kind === "int") value = Math.round(Number(raw));
    if (p.range.kind === "bool") value = Boolean(raw);
    applyEdit(
      [
        isMaster
          ? { op: "set_master_param", path: p.path, value }
          : { op: "set_param", track: t!.id, path: p.path, value },
      ],
      `${targetName} の ${p.display_name} を変更`,
    );
  }

  function fmtValue(p: ParamView, dragging?: number): string {
    // CLAP プラグインのつまみはプラグイン自身の表示を優先(ドラッグ中はその値)
    if (dragging === undefined && p.current_text) return p.current_text;
    const v = dragging ?? p.current;
    if (typeof v === "number") {
      const digits = p.range.kind === "int" ? 0 : Math.abs(v) >= 100 ? 0 : 2;
      return `${v.toFixed(digits)}${p.unit ?? ""}`;
    }
    return `${v}`;
  }

  // ---- スライダー: 位置(0〜SLIDER_MAX)と値の変換 ----
  // 周波数・時間のように広い範囲を持つパラメータは skew(< 1)で下側の分解能を上げる
  // (以前は線形で、カットオフ 40〜12000Hz のうち 200〜800Hz がスライダーの 5% しかなかった)。
  // 式は JUCE と同じ: 位置 = ((値 − 最小) / 幅)^skew
  const SLIDER_MAX = 1000;

  function toPos(p: ParamView, v: number): number {
    if (p.range.kind !== "float" && p.range.kind !== "int") return 0;
    const { min, max } = p.range;
    const t = Math.min(1, Math.max(0, (v - min) / (max - min || 1)));
    const skew = p.range.kind === "float" ? (p.range.skew ?? 1) : 1;
    return Math.round(Math.pow(t, skew) * SLIDER_MAX);
  }

  function fromPos(p: ParamView, pos: number): number {
    if (p.range.kind !== "float" && p.range.kind !== "int") return 0;
    const { min, max } = p.range;
    const skew = p.range.kind === "float" ? (p.range.skew ?? 1) : 1;
    const v = min + (max - min) * Math.pow(pos / SLIDER_MAX, 1 / skew);
    if (p.range.kind === "int") return Math.round(v);
    // 表示の桁に合わせて丸める(履歴に 0.30000000004 のような値を残さない)
    const digits = Math.abs(v) >= 100 ? 1 : 3;
    return Number(v.toFixed(digits));
  }

  /// ドラッグ中の値(パラメータのパス → 値)。離すまで表示だけ変える
  let dragValues = $state<Record<string, number>>({});

  function onSliderInput(p: ParamView, e: Event) {
    dragValues[p.path] = fromPos(p, Number((e.currentTarget as HTMLInputElement).value));
  }

  function onSliderChange(p: ParamView, e: Event) {
    const v = fromPos(p, Number((e.currentTarget as HTMLInputElement).value));
    delete dragValues[p.path];
    commitParam(p, v);
  }

  // ---- つまみのグループ(名前で分ける。知らない名前は「その他」) ----
  const GROUPS: [string, string[]][] = [
    ["音の元", ["waveform", "unison", "detune", "sub", "noise", "table", "position", "ratio", "feedback", "pick", "root", "tune"]],
    ["変調", ["index", "index_decay", "index_sustain", "pos_env", "pos_decay", "lfo_rate", "lfo_depth"]],
    ["音色", ["cutoff", "resonance", "filter_env", "tone", "brightness"]],
    ["エンベロープ", ["attack", "decay", "sustain", "release", "release_ms"]],
    ["奏法の効き", ["staccato", "accent", "vibrato", "bend", "legato", "portamento", "palm_mute"]],
    ["出力", ["gain_db"]],
  ];

  function grouped(params: ParamView[]): { name: string; params: ParamView[] }[] {
    const out = GROUPS.map(([name, keys]) => ({ name, params: params.filter((p) => keys.includes(p.name)) }));
    const known = new Set(GROUPS.flatMap(([, keys]) => keys));
    out.push({ name: "その他", params: params.filter((p) => !known.has(p.name)) });
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
    const on = !t[prop];
    applyEdit(
      [{ op: "set_track_prop", id: t.id, prop, value: on }],
      `${t.name} の${prop === "mute" ? "ミュート" : "ソロ"}を${on ? "オン" : "解除"}`,
    );
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
      showError("プリセットを保存できませんでした", e);
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
      `${t.name} から ${bus.name} への送りを ${levelDb.toFixed(1)} dB に`,
    );
  }

  function removeSend(bus: Track) {
    const t = track;
    if (!t) return;
    applyEdit([{ op: "set_send", track: t.id, target: bus.id }], `${t.name} から ${bus.name} への送りを外す`);
  }

  /// 送る量のドラッグ中の値(バス ID → dB)。離すまで表示だけ変える
  let sendDrag = $state<Record<string, number>>({});

  const sendSummary = $derived.by(() => {
    const t = track;
    if (!t || buses.length === 0) return "バスなし";
    const on = buses.filter((b) => t.sends?.some((s) => s.target === b.id));
    return on.length === 0 ? "送っていない" : on.map((b) => b.name).join("・");
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
    if (fx.name !== "clap") return fx.name;
    return fx.plugin_name ?? fx.plugin_id ?? "CLAP";
  }

  function openFxGui(fx: EffectView) {
    fxMenu = null;
    api.clapOpenGui(null, fx.id).catch((e) => showError("プラグインの画面を開けませんでした", e));
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
      `${targetName} に ${label} を追加`,
    );
  }

  function removeEffect(fx: EffectView) {
    fxMenu = null;
    applyEdit([{ op: "remove_effect", id: fx.id }], `${targetName} の ${fxLabel(fx)} を削除`);
  }

  function toggleBypass(fx: EffectView) {
    applyEdit(
      [{ op: "set_effect_bypass", id: fx.id, bypass: !fx.bypass }],
      `${targetName} の ${fxLabel(fx)} を${fx.bypass ? "有効に" : "バイパス"}`,
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
      fxPresetMsg = r.current_preset ? `今: ${r.current_preset}` : null;
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
    fxPresetMsg = "読み込み中…";
    try {
      const r = await api.clapLoadPreset(null, presetId, fx.id);
      fxPresetMsg = `今: ${r.preset}(Ctrl+Z で戻せます)`;
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

  const shownEffects = $derived.by((): EffectView[] => {
    const list = info?.effects ?? [];
    const order = fxOrder;
    if (!order) return list;
    const byId = new Map(list.map((f) => [f.id, f]));
    const out = order.map((id) => byId.get(id)).filter((f): f is EffectView => !!f);
    return out.length === list.length ? out : list;
  });

  $effect(() => {
    const order = fxOrder;
    if (order && (info?.effects ?? []).map((f) => f.id).join() === order.join()) fxOrder = null;
  });

  function onFxGripDown(e: PointerEvent, fx: EffectView, index: number) {
    if (e.button !== 0 || !fxList) return;
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
      fxOrder = ids;
      clearTimeout(fxOrderTimer);
      fxOrderTimer = setTimeout(() => (fxOrder = null), 3000);
      api
        .applyEdit([{ op: "move_effect", id: d.id, to_index: d.to }], `${targetName} の ${fxLabel(fx)} を ${d.to + 1} 番目へ`)
        .catch((e) => {
          fxOrder = null;
          loadError = String(e);
        });
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
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
      `${t.name} に試聴フレーズを挿入`,
    );
  }

  function removePhrase() {
    srcMenu = null;
    const t = track;
    const c = phraseClip;
    if (!t || !c) return;
    applyEdit([{ op: "remove_clip", id: c.id }], `${t.name} の試聴フレーズを削除`);
  }

  function closeMenus() {
    fxMenu = null;
    addMenu = null;
    srcMenu = null;
  }

  const KIND_LABEL = { midi: "MIDI", audio: "音声", bus: "バス" } as const;
</script>

{#snippet paramCell(p: ParamView)}
  <div class="pm" title={p.description}>
    <div class="pm-top">
      <span class="pm-name">{p.display_name}</span>
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
        onchange={(e) => onSliderChange(p, e)}
        aria-label={p.display_name}
      />
    {:else if p.range.kind === "bool"}
      <label class="pm-bool"
        ><input type="checkbox" checked={Boolean(p.current)} onchange={(e) => commitParam(p, (e.currentTarget as HTMLInputElement).checked)} />
        {p.current ? "オン" : "オフ"}</label
      >
    {:else}
      <select value={String(p.current)} onchange={(e) => commitParam(p, (e.currentTarget as HTMLSelectElement).value)} aria-label={p.display_name}>
        {#each p.range.choices as c (c)}
          <option value={c}>{c}</option>
        {/each}
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
  <aside class="sd-panel" style="width:{inspectorStore.width}px" aria-label="インスペクター">
    <div class="resize" role="separator" aria-orientation="vertical" title="ドラッグで幅を変える" onpointerdown={startResize}></div>
    <div class="sd-head">
      {#if isMaster}
        <Icon name="sliders-horizontal" />
        <b class="sd-title">マスター</b>
        <span class="sd-kind">曲全体に掛かるエフェクト</span>
      {:else if track}
        <span class="dot" style={track.color ? `background:${track.color}` : ""}></span>
        <b class="sd-title" title={track.name}>{track.name}</b>
        <span class="sd-kind">{KIND_LABEL[track.kind]}</span>
        <button class="btn letter" class:m-on={track.mute} onclick={() => toggleProp("mute")} title="ミュート" aria-pressed={track.mute}>M</button>
        <button class="btn letter" class:s-on={track.solo} onclick={() => toggleProp("solo")} title="ソロ(このトラックだけ聴く)" aria-pressed={track.solo}
          >S</button
        >
      {/if}
      <button class="btn sm icon ghost" onclick={close} title="閉じる" aria-label="閉じる"><Icon name="x" /></button>
    </div>

    {#if loadError}
      <div class="sd-error">{loadError}</div>
    {/if}

    <div class="sd-body">
      {#if track && track.kind === "bus"}
        <section class="sec">
          {@render secHead("bus", "バス", `受けている ${busSenders.length} 本`)}
          {#if !closed.bus}
            <div class="sec-b">
              <div class="hint">各トラックのインスペクターの「送り」で、このバスへ送る量を決めます。ここに挿したエフェクト(リバーブ・ディレイ等)を複数のトラックで共有できます。</div>
              {#each busSenders as s (s.name)}
                <div class="kv"><span>{s.name}</span><span>{s.level_db.toFixed(1)} dB{s.pre ? "・フェーダー前" : ""}</span></div>
              {:else}
                <div class="hint">まだどのトラックからも送られていません。</div>
              {/each}
            </div>
          {/if}
        </section>
      {/if}

      {#if track && track.kind === "midi"}
        <!-- 音源 -->
        <section class="sec">
          {@render secHead("src", "音源", deviceName(track.device))}
          {#if !closed.src}
            <div class="sec-b">
              <div class="src-card">
                <span class="src-ic"><Icon name={deviceIcon(track.device)} size={22} /></span>
                <span class="src-nm"
                  ><b>{track.device?.type === "clap" ? (info?.device.name ?? deviceName(track.device)) : deviceName(track.device)}</b><small
                    >{deviceKind(track.device)}{info?.device.is_default_fallback ? "(未設定なので既定の音)" : ""}</small
                  ></span
                >
                <button class="btn sm" onclick={(e) => openPicker(e)} title="音源を変える(内蔵・マイプリセット・SoundFont・CLAP・サンプル)"
                  >変更<Icon name="chevron-down" /></button
                >
              </div>
              <div class="row">
                <button class="btn sm" onclick={(e) => openPicker(e, "preset")} title="保存したプリセット(全プロジェクト共通)から選ぶ"
                  ><Icon name="save" />プリセットから</button
                >
                {#if track.device?.type !== "clap"}
                  <button class="btn sm" class:on={savingPreset} onclick={() => (savingPreset = !savingPreset)} title="今の音(音源とエフェクト)をプリセットとして保存"
                    ><Icon name="plus" />保存</button
                  >
                {:else}
                  <button class="btn sm" onclick={() => api.clapOpenGui(track!.id).catch((e) => showError("プラグインの画面を開けませんでした", e))}
                    ><Icon name="app-window" />プラグインの画面</button
                  >
                {/if}
                <span class="grow"></span>
                <button class="btn sm icon ghost" onclick={(e) => (srcMenu = menuAt(e))} title="その他(試聴フレーズなど)" aria-label="その他"
                  ><Icon name="ellipsis" /></button
                >
              </div>
              {#if savingPreset}
                <div class="row">
                  <!-- svelte-ignore a11y_autofocus -->
                  <input
                    class="grow"
                    type="text"
                    placeholder="プリセットの名前"
                    autofocus
                    bind:value={presetName}
                    onkeydown={(e) => {
                      if (e.key === "Enter" && !e.isComposing) {
                        e.preventDefault();
                        savePreset();
                      } else if (e.key === "Escape") savingPreset = false;
                    }}
                  />
                  <button class="btn sm primary" disabled={!presetName.trim()} onclick={savePreset}>保存</button>
                </div>
              {/if}

              {#if track.device?.type === "clap"}
                <!-- CLAP: 音作りはプラグイン自身の画面で。ここではプリセットを選ぶ -->
                <div class="grp-t">
                  プリセット{currentClapPreset ? `(今: ${currentClapPreset})` : ""}
                  <button class="btn sm icon ghost" disabled={clapPresetBusy} title="プリセットを探し直す(追加した後など)" aria-label="探し直す" onclick={() => track && loadClapPresets(track.id, true)}
                    ><Icon name="refresh-cw" /></button
                  >
                  <button
                    class="btn sm icon ghost"
                    title="プラグインの今の設定をプロジェクトに保存する(画面を閉じたときや操作の後にも自動で保存されます)"
                    aria-label="設定を保存"
                    onclick={() => api.clapSaveState(track!.id).catch((e) => showError("プラグインの状態を保存できませんでした", e))}
                    ><Icon name="save" /></button
                  >
                </div>
                {#if clapPresetList === null}
                  <div class="hint">プリセットを探しています…</div>
                {:else if clapPresetList.length === 0}
                  <div class="hint">このプラグインはプリセットを Glaux に公開していません。プラグインの画面のプリセットメニューから選んでください(選んだ設定は自動でプロジェクトに保存され、Ctrl+Z で戻せます)。</div>
                {:else}
                  <div class="row">
                    <select bind:value={clapCategory} title="カテゴリ(フォルダ)">
                      <option value="">すべて({clapPresetList.length})</option>
                      {#each clapCategories as c (c.name)}
                        <option value={c.name}>{c.name || "(未分類)"}({c.count})</option>
                      {/each}
                    </select>
                    <input class="grow" type="search" placeholder="名前で絞り込み" bind:value={clapSearch} />
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
                      <div class="hint">…ほか {shownClapPresets.length - 500} 件(絞り込んでください)</div>
                    {/if}
                  </div>
                {/if}
                {#if clapPresetMsg}<div class="hint">{clapPresetMsg}</div>{/if}
              {:else if info}
                {#each grouped(info.params) as g (g.name)}
                  <div class="grp-t">{g.name}</div>
                  <div class="params">
                    {#each g.params as p (p.path)}
                      {@render paramCell(p)}
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
          {@render secHead("fx", "エフェクト", shownEffects.length === 0 ? "なし" : shownEffects.map((f) => fxLabel(f)).join(" → "))}
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
                      <span class="grip" role="button" tabindex="-1" aria-label="並べ替え" title="つかんで上下にドラッグで並べ替え" onpointerdown={(e) => onFxGripDown(e, fx, i)}
                        ><Icon name="grip-vertical" size={14} /></span
                      >
                      <button
                        class="btn sm icon"
                        class:on={!fx.bypass}
                        onclick={() => toggleBypass(fx)}
                        title={fx.bypass ? "バイパス中(クリックで有効に)" : "有効(クリックでバイパス)"}
                        aria-label="有効 / バイパス"
                        aria-pressed={!fx.bypass}><Icon name="power" /></button
                      >
                      <button class="fx-name" onclick={() => (fxFolded[fx.id] = !fxFolded[fx.id])} title={fx.plugin_id ?? "クリックで開く / 畳む"}>
                        <b>{fxLabel(fx)}</b>{#if fx.name === "clap"}<span class="clap-chip">CLAP</span>{/if}
                      </button>
                      {#if fx.name === "clap" && !fx.missing}
                        <button class="btn sm icon ghost" onclick={() => openFxGui(fx)} title="プラグインの画面を開く" aria-label="プラグインの画面"
                          ><Icon name="app-window" /></button
                        >
                      {/if}
                      <button class="btn sm icon ghost" onclick={() => (fxFolded[fx.id] = !fxFolded[fx.id])} aria-label={fxFolded[fx.id] ? "開く" : "畳む"}
                        ><Icon name={fxFolded[fx.id] ? "chevron-right" : "chevron-down"} /></button
                      >
                      <button class="btn sm icon ghost" onclick={(e) => (fxMenu = { fx, ...menuAt(e) })} title="その他(プリセット・削除)" aria-label="その他"
                        ><Icon name="ellipsis" /></button
                      >
                    </div>
                    {#if !fxFolded[fx.id]}
                      <div class="fx-b">
                        {#if fxPresetOpen === fx.id}
                          <div class="fx-presets">
                            {#if fxPresetList === null}
                              <div class="hint">プリセットを探しています…</div>
                            {:else if fxPresetList.length === 0}
                              <div class="hint">このプラグインはプリセットを Glaux に公開していません(例: Surge XT Effects)。プラグインの画面のプリセットメニューから選んでください。選んだ設定は自動でプロジェクトに保存され、Ctrl+Z で戻せます。</div>
                            {:else}
                              <select class="grow" disabled={fxPresetBusy} onchange={(e) => loadFxPreset(fx, (e.currentTarget as HTMLSelectElement).value)}>
                                <option value="">プリセットを選ぶ…({fxPresetList.length})</option>
                                {#each fxPresetGroups as [cat, list] (cat)}
                                  <optgroup label={cat || "(未分類)"}>
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
                          <div class="hint warn">このプラグインはこの PC に見つかりません(音は素通し)。インストールすると元の設定で鳴ります。</div>
                        {/if}
                        <div class="params">
                          {#each fx.params as p (p.path)}
                            {@render paramCell(p)}
                          {/each}
                        </div>
                        {#if fx.name === "clap" && (fx.param_total ?? 0) > fx.params.length}
                          <div class="hint">ほか {(fx.param_total ?? 0) - fx.params.length} 個のつまみはプラグインの画面か AI から操作できます。</div>
                        {/if}
                      </div>
                    {/if}
                  </div>
                {/each}
              </div>
              <button class="btn sm add-fx" onclick={(e) => (addMenu = menuAt(e))}><Icon name="plus" />エフェクトを追加<Icon name="chevron-down" /></button>
            </div>
          {/if}
        </section>
      {/if}

      {#if track && track.kind !== "bus"}
        <!-- 送り(バスへ送る量) -->
        <section class="sec">
          {@render secHead("send", "送り", sendSummary)}
          {#if !closed.send}
            <div class="sec-b">
              {#each buses as bus (bus.id)}
                {@const snd = track.sends?.find((s) => s.target === bus.id)}
                {@const shown = sendDrag[bus.id] ?? snd?.level_db}
                <div class="send" title="このトラックの音をバスへ送る量。フェーダー後はトラックの音量に追従します">
                  <span class="send-nm">{bus.name}</span>
                  <input
                    type="range"
                    min="-60"
                    max="6"
                    step="0.5"
                    value={snd?.level_db ?? -60}
                    aria-label={`${bus.name} へ送る量`}
                    oninput={(e) => (sendDrag[bus.id] = Number((e.currentTarget as HTMLInputElement).value))}
                    onchange={(e) => {
                      delete sendDrag[bus.id];
                      setSend(bus, Number((e.currentTarget as HTMLInputElement).value), snd?.pre_fader ?? false);
                    }}
                  />
                  <span class="pm-val">{shown !== undefined ? `${shown.toFixed(1)} dB` : "送らない"}</span>
                  <button
                    class="btn sm icon ghost"
                    class:on={snd?.pre_fader}
                    disabled={!snd}
                    onclick={() => snd && setSend(bus, snd.level_db, !(snd.pre_fader ?? false))}
                    title={snd?.pre_fader ? "フェーダー前から送っている(クリックでフェーダー後に)" : "フェーダー後から送っている(クリックでフェーダー前に)"}
                    aria-label="フェーダー前から送る"><Icon name="arrow-up" /></button
                  >
                  <button class="btn sm icon ghost" disabled={!snd} onclick={() => removeSend(bus)} title="送らない" aria-label="送らない"
                    ><Icon name="x" /></button
                  >
                </div>
              {:else}
                <div class="hint">バスがありません。タイムラインの「トラックを追加」からバスを作ると、複数のトラックで同じリバーブを共有できます。</div>
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
            "つなぎ",
            `滑る時間 ${track.glide_ms ?? 150}ms · つなぎ目 ${track.legato_ms ?? 30}ms${track.glide_ms === undefined && track.legato_ms === undefined ? "(既定)" : ""}`,
          )}
          {#if !closed.legato}
            <div class="sec-b">
              <div class="params">
                <div class="pm" title="ポルタメント(P)のノートが直前の音から滑る時間。ゆったりした弦は 250〜400、速いリードは 50〜80">
                  <div class="pm-top"><span class="pm-name">滑る時間</span><span class="pm-val">{track.glide_ms ?? 150}ms</span></div>
                  <input type="range" min="10" max="1000" step="10" value={track.glide_ms ?? 150} aria-label="滑る時間"
                    onchange={(e) => setLegato("glide_ms", Number((e.currentTarget as HTMLInputElement).value))} />
                </div>
                <div class="pm" title="レガート(T)・ポルタメント(P)で前の音と入れ替わる長さ。長いほどふんわり重なる">
                  <div class="pm-top"><span class="pm-name">つなぎ目</span><span class="pm-val">{track.legato_ms ?? 30}ms</span></div>
                  <input type="range" min="5" max="200" step="5" value={track.legato_ms ?? 30} aria-label="つなぎ目"
                    onchange={(e) => setLegato("legato_ms", Number((e.currentTarget as HTMLInputElement).value))} />
                </div>
              </div>
              <div class="row">
                <span class="hint grow">ピアノロールでノートに T(レガート)/ P(ポルタメント)を付けたときのつながり方</span>
                {#if track.glide_ms !== undefined || track.legato_ms !== undefined}
                  <button class="btn sm" onclick={resetLegato}>既定に戻す</button>
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
        <button onclick={() => toggleFxPresets(fx)}><Icon name="save" />プリセットから選ぶ</button>
        <button onclick={() => openFxGui(fx)}><Icon name="app-window" />プラグインの画面を開く</button>
        <div class="menu-sep"></div>
      {/if}
      <button class="danger" onclick={() => removeEffect(fx)}><Icon name="trash-2" />削除<span class="key">Ctrl+Z で戻せます</span></button>
    </div>
  {/if}
  {#if addMenu && info}
    <div class="menu" use:keepInView style="left:{addMenu.x}px;top:{addMenu.y}px">
      <div class="menu-h">内蔵</div>
      {#each info.available_effects as fx (fx.name)}
        <button class="rich" onclick={() => addEffect(fx.name)}><span>{fx.name}<small>{fx.description}</small></span></button>
      {/each}
      {#if clapEffects.length > 0}
        <div class="menu-sep"></div>
        <div class="menu-h">CLAP プラグイン</div>
        {#each clapEffects as p (p.id)}
          <button class="rich" onclick={() => addEffect(`clap:${p.id}`)}><Icon name="plug" /><span>{p.name}<small>{p.vendor} {p.version}</small></span></button>
        {/each}
      {/if}
    </div>
  {/if}
  {#if srcMenu && track}
    <div class="menu" use:keepInView style="left:{srcMenu.x}px;top:{srcMenu.y}px">
      {#if phraseClip}
        <button onclick={removePhrase}><Icon name="trash-2" />試聴フレーズを消す</button>
      {:else}
        <button class="rich" onclick={insertPhrase} title="ロングトーン → 刻み → 分散和音 → オクターブ上の 4 小節"
          ><Icon name="music" /><span>試聴フレーズを入れる<small>末尾に 4 小節。再生とループを回しながら調整する</small></span></button
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
