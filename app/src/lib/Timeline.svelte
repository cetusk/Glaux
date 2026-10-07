<script lang="ts">
  import { designSel, designStore } from "./design.svelte";
  import { tick, untrack } from "svelte";
  import * as api from "./api";
  import { shouldYieldKey } from "./keys";
  import { aiHighlight } from "./aiHighlight.svelte";
  import { harmonyStore } from "./harmony.svelte";
  import { showError, showToast } from "./toast.svelte";
  import { plural, tr } from "./i18n.svelte";
  import AutomationLaneRow from "./AutomationLaneRow.svelte";
  import { barAtTick, barsEndTick, buildBars, fmtBpm, meterLabel } from "./barMap";
  import AudioClipPreview from "./AudioClipPreview.svelte";
  import ClipPreview from "./ClipPreview.svelte";
  import SimilarPresetDialog from "./SimilarPresetDialog.svelte";
  import RulerMenu from "./RulerMenu.svelte";
  import ClipMenu from "./ClipMenu.svelte";
  import TrackMenu from "./TrackMenu.svelte";
  import AddTrackMenu from "./AddTrackMenu.svelte";
  import AudioClipHandles from "./AudioClipHandles.svelte";
  import TimelineZoom from "./TimelineZoom.svelte";
  import TimelineSections from "./TimelineSections.svelte";
  import {
    KIND_ICON,
    addClipAt,
    addTrack,
    clipMarkTitle,
    importAudioAt,
    importMidiFile,
    kindLabel,
    renameTrack,
    sendersOf,
    setMasterVolume,
    setTrackColor,
    setTrackVolume,
    toggleTrackFlag,
  } from "./trackActions";
  import { commitSections, markersOf, removeMarker } from "./sectionOps";
  import {
    bpmAt,
    cloneClip,
    expandLoopCommand,
    focusSelect,
    followBpm,
    planText,
    type ClipMenuAction,
    type Marker,
    type RulerMenuState,
  } from "./timelineOps";
  import { newClipId } from "./ids";
  import {
    instrumentPickerStore,
    MASTER_FOCUS_ID,
    midiArmStore,
    pianoRollStore,
    selectionStore,
    soundDesignStore,
    saveTimelineLayout,
    layoutMax,
    TIMELINE_HEAD_W,
    TIMELINE_TRACK_H,
    timelineLayout,
    timelineZoom,
    TIMELINE_ZOOM_MAX,
    TIMELINE_ZOOM_MIN,
    viewStore,
  } from "./selection.svelte";
  import Icon from "./Icon.svelte";
  import { flip } from "svelte/animate";
  import { deviceIcon, deviceName } from "./instruments";
  import type { Clip, Project, Track } from "./types";

  let {
    project,
    playheadTick = 0,
    playing = false,
    onSeek,
  }: {
    project: Project;
    playheadTick?: number;
    playing?: boolean;
    onSeek?: (tick: number) => void;
  } = $props();

  // 拡大率 1 で 4/4 の 1 小節 = 96px となる密度。小節の実幅は拍子に応じて変わる(7/8 は狭い)
  const PX_PER_WHOLE = 96;
  const pxPerTick = $derived((PX_PER_WHOLE * timelineZoom.value) / (project.ppq * 4));
  /// 小節番号を何小節おきに出すか(詰まって重ならないように、番号の間を 28px 以上あける)
  const barLabelEvery = $derived.by(() => {
    const barPx = PX_PER_WHOLE * timelineZoom.value;
    for (const n of [1, 2, 4, 8, 16, 32]) if (barPx * n >= 28) return n;
    return 64;
  });

  const endTick = $derived.by(() => {
    let end = 0;
    for (const t of project.tracks) {
      for (const c of t.clips) {
        end = Math.max(end, c.start + c.length);
      }
    }
    return end;
  });

  // 拍子イベントを考慮した小節列(グリッド・ルーラー・範囲選択の共通ソース)
  const barList = $derived(buildBars(project, endTick));
  const totalPx = $derived(barsEndTick(barList) * pxPerTick);
  // レーンの小節線: トラックごとに小節の数だけ要素を作らず、全レーン共通の背景画像(SVG)1 枚にする
  const gridImage = $derived.by(() => {
    const w = Math.max(1, Math.ceil(totalPx));
    const lines = barList
      .map((b) => `<rect x='${Math.round(b.tick * pxPerTick)}' width='1' height='1'/>`)
      .join("");
    const svg = `<svg xmlns='http://www.w3.org/2000/svg' width='${w}' height='1' viewBox='0 0 ${w} 1' preserveAspectRatio='none' shape-rendering='crispEdges'><g fill='rgba(255,255,255,0.05)'>${lines}</g></svg>`;
    return `url("data:image/svg+xml,${encodeURIComponent(svg)}")`;
  });

  function clipStyle(clip: Clip): string {
    let start = clip.start;
    let length = clip.length;
    if (clipDrag?.moved && clipDrag.clip.id === clip.id) {
      start = clipDrag.previewStart;
      length = clipDrag.previewLength;
    } else if (clipDrag?.moved && clipDrag.mode === "move" && clipDrag.group.has(clip.id)) {
      // 複数選択の一括移動: 掴んだクリップと同じだけずらす
      start = Math.max(0, clip.start + (clipDrag.previewStart - clipDrag.clip.start));
    }
    const left = start * pxPerTick;
    const width = Math.max(length * pxPerTick, 8);
    return `left:${left}px;width:${width}px`;
  }

  /** 見出しの音量(dB は title で。幅を取らないよう数字だけ) */
  function volumeText(t: Track): string {
    const v = t.volume_db;
    return `${v > 0 ? "+" : ""}${v.toFixed(1)}`;
  }

  /// 見出しの横幅とトラックの高さ(ルーラー左の「表示」で変える)
  const HEAD_W = $derived(Math.min(timelineLayout.headW, layoutMax("w")));
  /// 既定のトラックの高さ(ウィンドウの高さの半分までに抑えた値)
  const defaultTrackH = $derived(Math.min(timelineLayout.trackH, layoutMax("h")));

  /// 見出しの幅のつかむ所にマウスが乗っている・ドラッグ中(幅は全トラック共通なので、列全体を光らせる)
  let wGripHot = $state(false);

  /// トラックの高さ(個別に変えていなければ既定の高さ)
  function trackHeight(id: string): number {
    return Math.min(timelineLayout.trackHeights[id] ?? timelineLayout.trackH, layoutMax("h"));
  }

  /// 見出しの右の端(幅。全トラック共通)・下の端(そのトラックの高さ)をドラッグして変える
  function startLayoutDrag(e: PointerEvent, what: "w" | "h", trackId?: string) {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    const start = what === "w" ? e.clientX : e.clientY;
    const orig = what === "w" ? HEAD_W : trackHeight(trackId ?? "");
    const r = { min: (what === "w" ? TIMELINE_HEAD_W : TIMELINE_TRACK_H).min, max: layoutMax(what) };
    if (what === "w") wGripHot = true;
    const move = (ev: PointerEvent) => {
      const d = (what === "w" ? ev.clientX : ev.clientY) - start;
      const v = Math.round(Math.min(r.max, Math.max(r.min, orig + d)));
      if (what === "w") timelineLayout.headW = v;
      else if (trackId) timelineLayout.trackHeights[trackId] = v;
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      document.body.classList.remove(what === "w" ? "resizing-w" : "resizing-h");
      if (what === "w") wGripHot = false;
      saveTimelineLayout();
    };
    document.body.classList.add(what === "w" ? "resizing-w" : "resizing-h");
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  function resetLayout(what: "w" | "h", trackId?: string) {
    if (what === "w") timelineLayout.headW = TIMELINE_HEAD_W.def;
    else if (trackId) delete timelineLayout.trackHeights[trackId];
    saveTimelineLayout();
  }

  // ---- 横の拡大・縮小(Ctrl+ホイールはカーソルの下の位置を保つ。ボタンは画面の左端を保つ) ----

  /// 拡大率を変える。`anchorX` はスクローラーの左端からの画面上の位置(そこにある時刻を動かさない)
  function setZoom(next: number, anchorX?: number) {
    const scroller = root?.parentElement;
    const z = Math.min(TIMELINE_ZOOM_MAX, Math.max(TIMELINE_ZOOM_MIN, next));
    if (!scroller || z === timelineZoom.value) {
      timelineZoom.value = z;
      return;
    }
    const ax = anchorX ?? HEAD_W;
    const tickAt = Math.max(0, (scroller.scrollLeft + ax - HEAD_W) / pxPerTick);
    timelineZoom.value = z;
    // 幅が変わったあとで位置を合わせる
    tick().then(() => {
      scroller.scrollLeft = Math.max(0, HEAD_W + tickAt * pxPerTick - ax);
    });
  }

  function zoomBy(factor: number) {
    setZoom(timelineZoom.value * factor);
  }

  /// 曲全体(クリップのある所まで)が横に収まる拡大率にする
  function zoomToFit() {
    const scroller = root?.parentElement;
    if (!scroller) return;
    const end = Math.max(endTick, project.ppq * 4 * 4);
    const avail = Math.max(100, scroller.clientWidth - HEAD_W - 24);
    setZoom((avail / end) * ((project.ppq * 4) / PX_PER_WHOLE));
    tick().then(() => (scroller.scrollLeft = 0));
  }

  function onWheel(e: WheelEvent) {
    if (!(e.ctrlKey || e.metaKey)) return;
    e.preventDefault();
    const scroller = root?.parentElement;
    if (!scroller) return;
    const x = e.clientX - scroller.getBoundingClientRect().left;
    if (x < HEAD_W) return;
    // ホイール 1 目盛り(100)で約 1.25 倍。タッチパッドの細かい量にもなめらかに
    setZoom(timelineZoom.value * Math.exp(-e.deltaY * 0.0022), x);
  }

  $effect(() => {
    const el = root;
    if (!el) return;
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  });
  /// セクション行の高さ(18px + 下線 1px)。ルーラーはこの下に固定する
  const SECTION_ROW_H = 19;
  const playheadPx = $derived(HEAD_W + playheadTick * pxPerTick);

  // ---- 再生中の自動スクロール(再生ヘッドが見える範囲を追いかける) ----

  let root: HTMLDivElement | undefined = $state();

  /// 横のスクロール量。再生ヘッドが見出しの列に入ったら描かない(見出しの隙間から透けて見えないように)
  let scrollX = $state(0);
  $effect(() => {
    const scroller = root?.parentElement;
    if (!scroller) return;
    const onScroll = () => (scrollX = scroller.scrollLeft);
    onScroll();
    scroller.addEventListener("scroll", onScroll, { passive: true });
    return () => scroller.removeEventListener("scroll", onScroll);
  });

  let lastFollowPx: number | null = null;

  $effect(() => {
    const px = playheadPx; // 依存として追跡
    void playing;
    // 位置が変わったときだけ追従(停止中のシークにも反応し、手動スクロールは邪魔しない)
    if (px === lastFollowPx) return;
    const first = lastFollowPx === null;
    lastFollowPx = px;
    if (first) return;
    const scroller = root?.parentElement;
    if (!scroller) return;
    const view = scroller.clientWidth;
    const left = scroller.scrollLeft;
    // 右端に近づいたら(またはヘッドが画面外なら)ページ送り
    if (px > left + view - 80 || px < left + HEAD_W) {
      scroller.scrollLeft = Math.max(0, px - HEAD_W - 80);
    }
  });

  // ---- ルーラー: クリックでシーク、ドラッグで小節範囲を選択(マスク) ----

  let dragStart: { x: number; bar: number } | null = null;
  let dragging = false;

  function barAt(e: PointerEvent): number {
    return barAtX(e.currentTarget as HTMLElement, e.clientX);
  }

  function barAtX(lane: HTMLElement, clientX: number): number {
    const x = clientX - lane.getBoundingClientRect().left;
    return barAtTick(barList, Math.max(0, x / pxPerTick)).index;
  }

  // ---- 範囲選択のドラッグで、見えている範囲の端を越えたら自動でスクロールする ----
  let autoScroll: { lane: HTMLElement; clientX: number; over: number } | null = null;
  let autoRaf = 0;
  /** 端からこの距離(px)に入ったらスクロールを始める */
  const AUTO_EDGE = 24;

  function kickAutoScroll(lane: HTMLElement, clientX: number) {
    const scroller = root?.parentElement;
    if (!scroller) return;
    const r = scroller.getBoundingClientRect();
    const left = r.left + HEAD_W + AUTO_EDGE;
    const right = r.right - AUTO_EDGE;
    const over = clientX < left ? clientX - left : clientX > right ? clientX - right : 0;
    autoScroll = { lane, clientX, over };
    if (over !== 0 && !autoRaf) autoRaf = requestAnimationFrame(stepAutoScroll);
  }

  function stepAutoScroll() {
    autoRaf = 0;
    const a = autoScroll;
    const scroller = root?.parentElement;
    if (!a || a.over === 0 || !dragStart || !dragging || !scroller) return;
    // 越えた量が大きいほど速く(1 コマ 4〜40px)
    const speed = Math.sign(a.over) * Math.min(40, 4 + Math.abs(a.over) * 0.5);
    const before = scroller.scrollLeft;
    scroller.scrollLeft = Math.max(0, before + speed);
    if (scroller.scrollLeft !== before) setRange(dragStart.bar, barAtX(a.lane, a.clientX));
    autoRaf = requestAnimationFrame(stepAutoScroll);
  }

  function stopAutoScroll() {
    autoScroll = null;
    if (autoRaf) cancelAnimationFrame(autoRaf);
    autoRaf = 0;
  }

  function setRange(a: number, b: number) {
    const startBar = Math.min(a, b);
    const endBar = Math.max(a, b);
    const s = barList[Math.min(startBar, barList.length - 1)];
    const e = barList[Math.min(endBar, barList.length - 1)];
    selectionStore.range = {
      startBar,
      endBar,
      startTick: s.tick,
      endTick: e.tick + e.len,
    };
  }

  function onRulerDown(e: PointerEvent) {
    dragStart = { x: e.clientX, bar: barAt(e) };
    dragging = false;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }

  function onRulerMove(e: PointerEvent) {
    if (!dragStart) return;
    if (!dragging && Math.abs(e.clientX - dragStart.x) > 4) {
      dragging = true;
    }
    if (dragging) {
      setRange(dragStart.bar, barAt(e));
      kickAutoScroll(e.currentTarget as HTMLElement, e.clientX);
    }
  }

  function onRulerUp(e: PointerEvent) {
    stopAutoScroll();
    if (!dragStart) return;
    if (!dragging) {
      // クリック = シーク + 選択解除
      selectionStore.range = null;
      const lane = e.currentTarget as HTMLElement;
      const x = e.clientX - lane.getBoundingClientRect().left;
      onSeek?.(Math.max(0, x / pxPerTick));
    }
    dragStart = null;
    dragging = false;
  }

  const selection = $derived(selectionStore.range);

  // ---- 拍子の変更(ルーラー右クリック / 拍子チップのクリック) ----

  let sigMenu = $state<RulerMenuState | null>(null);

  // ---- セクションマーカー(曲の構成)。以前は AI の set_sections でしか置けなかった ----

  /// 小節頭 tick → コード名(ノートからの推定。前の小節と同じ・ノートなしは出さない)
  const chordAt = $derived.by(() => {
    const m = new Map<number, string>();
    let prev = "";
    for (const c of harmonyStore.view?.chords ?? []) {
      if (c.chord !== "N.C." && c.chord !== prev) m.set(c.tick, c.chord);
      prev = c.chord;
    }
    return m;
  });
  /// マーカーの名前の入力欄(ルーラーのメニュー。閉じても残す)
  let markerName = $state("");

  /// 小節の頭にマーカーを置く(既にあれば名前を変える)
  function putMarker(barIndex: number, name: string) {
    const n = name.trim();
    if (!n) return;
    const tick = barList[Math.min(barIndex, barList.length - 1)].tick;
    const list = markersOf(project);
    const hit = list.find((m) => m.tick === tick);
    if (hit) hit.name = n;
    else list.push({ tick, name: n });
    sigMenu = null;
    markerName = "";
    commitSections(
      list,
      hit
        ? tr(`マーカーの名前を「${n}」に変更`, `Rename marker to "${n}"`)
        : tr(`${barIndex + 1} 小節目にマーカー「${n}」を追加`, `Add marker "${n}" at bar ${barIndex + 1}`),
    );
  }

  function removeMarkerAt(tick: number) {
    if (removeMarker(project, tick)) sigMenu = null;
  }

  function openSigMenu(e: MouseEvent, barIndex: number) {
    e.preventDefault();
    const bar = barList[Math.min(barIndex, barList.length - 1)];
    sigMenu = {
      x: e.clientX,
      y: e.clientY,
      barIndex: bar.index,
      num: String(bar.num),
      den: String(bar.den),
      grouping: bar.grouping.join("+"),
      bpm: String(bpmAt(project, bar.tick)),
    };
  }

  // ---- 曲の途中のテンポ変更(ルーラーの右クリック。小節の頭から) ----

  /// 小節頭 tick → その小節から始まるテンポ(先頭以外。ルーラーに札を出す)
  const tempoAt = $derived(new Map(project.tempo_map.filter((e) => e.tick > 0).map((e) => [e.tick, e.bpm])));

  function onRulerContext(e: MouseEvent) {
    const lane = e.currentTarget as HTMLElement;
    const x = e.clientX - lane.getBoundingClientRect().left;
    openSigMenu(e, barAtTick(barList, Math.max(0, x / pxPerTick)).index);
  }

  // ---- トラック操作(Command API 経由、author: human) ----

  function setVolume(t: Track, e: Event) {
    setTrackVolume(t, Number((e.currentTarget as HTMLInputElement).value));
  }

  function toggleMute(t: Track) {
    // 選んだトラックのどれかを押したら、選んだ全部を押したトラックの新しい状態にそろえる
    toggleTrackFlag("mute", t, targetsOf(t));
  }

  function openPianoRoll(track: Track, clip: Clip, e: MouseEvent) {
    if (clip.kind !== "midi" || suppressOpen) return;
    pianoRollStore.focus = {
      clipId: clip.id,
      clipName: clip.name,
      trackId: track.id,
      trackName: track.name,
      anchorTick: Math.max(0, (e.offsetX ?? 0) / pxPerTick),
    };
  }

  /// 音声クリップを譜起こしして MIDI クリップにし、ピアノロールで開く
  /// (melody = 単旋律、poly = 和音)
  let transcribing = $state<string | null>(null);
  async function transcribe(track: Track, clip: Clip, mode: "melody" | "poly" = "melody") {
    if (transcribing) return;
    transcribing = clip.id;
    try {
      const r = await api.transcribeClip(clip.id, null, 240, mode);
      pianoRollStore.focus = {
        clipId: r.clip_id,
        clipName: `${clip.name} (MIDI)`,
        trackId: r.track_id,
        trackName: r.created_track ? `${track.name} MIDI` : track.name,
        anchorTick: 0,
      };
    } catch (e) {
      showError(tr("譜起こしできませんでした", "Couldn't transcribe"), e);
    } finally {
      transcribing = null;
    }
  }

  /// 音声クリップをパートに分離する(時間がかかるのでクリップに「分離中…」を出す)
  let separating = $state<string | null>(null);
  async function separate(clip: Clip, method: "builtin" | "demucs") {
    if (separating || clip.kind !== "audio") return;
    separating = clip.id;
    try {
      await api.separateClip(clip.id, method);
    } catch (e) {
      showError(tr("パートに分けられませんでした", "Couldn't separate into parts"), e);
    } finally {
      separating = null;
    }
  }

  /// 空きレーンのダブルクリック: その小節にクリップを作ってピアノロールを開く
  /// (音声トラックなら WAV を選んで配置)
  function onLaneDblClick(e: MouseEvent, track: Track) {
    if (suppressOpen) return;
    if ((e.target as HTMLElement).closest(".clip")) return; // 既存クリップは openPianoRoll 側
    const lane = e.currentTarget as HTMLElement;
    const x = e.clientX - lane.getBoundingClientRect().left;
    const tick = Math.max(0, x / pxPerTick);
    const bar = barAtTick(barList, tick);
    if (track.kind === "bus") return; // バスにはクリップを置かない
    if (track.kind === "audio") {
      void importAudioAt(track, bar.tick);
      return;
    }
    addClipAt(track, barList, tick)
      .then((c) => {
        if (!c) return;
        pianoRollStore.focus = {
          clipId: c.clipId,
          clipName: c.name,
          trackId: track.id,
          trackName: track.name,
          anchorTick: 0,
        };
      })
      .catch(() => {});
  }

  // ---- クリップのドラッグ編集(本体で移動、右端でリサイズ) ----

  let clipDrag = $state<{
    clip: Clip;
    trackId: string;
    mode: "move" | "resize";
    startX: number;
    startY: number;
    moved: boolean;
    previewStart: number;
    previewLength: number;
    previewTrackId: string;
    /// 一緒に動かす選択中のクリップ(掴んだクリップを含む)
    group: Set<string>;
  } | null>(null);
  /// ドラッグ直後の dblclick でピアノロールが開かないようにする
  let suppressOpen = false;

  function onClipDown(e: PointerEvent, track: Track, clip: Clip) {
    if (e.button !== 0) return;
    // Ctrl / Shift クリック: 選択に追加・除外(ドラッグはしない)
    if (e.ctrlKey || e.metaKey || e.shiftKey) {
      const next = new Set(selectedClips);
      if (next.has(clip.id)) next.delete(clip.id);
      else next.add(clip.id);
      selectedClips = next;
      return;
    }
    if (!selectedClips.has(clip.id)) selectedClips = new Set([clip.id]);
    const mode = (e.target as HTMLElement).classList.contains("clip-resize") ? "resize" : "move";
    clipDrag = {
      clip,
      trackId: track.id,
      mode,
      startX: e.clientX,
      startY: e.clientY,
      moved: false,
      previewStart: clip.start,
      previewLength: clip.length,
      previewTrackId: track.id,
      group: new Set(selectedClips),
    };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }

  function onClipDragMove(e: PointerEvent) {
    const d = clipDrag;
    if (!d) return;
    if (!d.moved && Math.abs(e.clientX - d.startX) < 4 && Math.abs(e.clientY - d.startY) < 4) {
      return;
    }
    d.moved = true;
    const snap = e.altKey ? 1 : project.ppq; // 1 拍スナップ(Alt で解除)
    const dxTick = (e.clientX - d.startX) / pxPerTick;
    if (d.mode === "resize") {
      const raw = d.clip.length + dxTick;
      d.previewLength = Math.max(240, Math.round(raw / snap) * snap);
    } else {
      const raw = d.clip.start + dxTick;
      // 一括移動では、いちばん左のクリップが 0 より前に出ないように抑える
      const minStart = Math.min(
        ...allClips().filter((c) => d.group.has(c.clip.id)).map((c) => c.clip.start),
      );
      const lowest = d.clip.start - minStart;
      d.previewStart = Math.max(lowest, Math.round(raw / snap) * snap);
      // 縦方向(トラック移動)は 1 つだけ動かすときに限る
      if (d.group.size > 1) return;
      // 縦方向: ポインタ直下のレーンが同種トラックなら移動先にする
      const lane = (document.elementFromPoint(e.clientX, e.clientY) as HTMLElement | null)?.closest(
        ".lane",
      ) as HTMLElement | null;
      const tid = lane?.dataset.trackId;
      if (tid) {
        const t = project.tracks.find((t) => t.id === tid);
        if (t && t.kind === (d.clip.kind === "midi" ? "midi" : "audio")) {
          d.previewTrackId = tid;
        }
      }
    }
  }

  function onClipUp() {
    const d = clipDrag;
    clipDrag = null;
    if (!d) return;
    if (!d.moved) {
      // クリックだけ: 複数選択中でもそのクリップ 1 つの選択に絞る
      selectedClips = new Set([d.clip.id]);
      return;
    }
    if (d.mode === "move" && d.group.size > 1) {
      const delta = d.previewStart - d.clip.start;
      if (delta === 0) return;
      suppressOpen = true;
      setTimeout(() => (suppressOpen = false), 400);
      const cmds = allClips()
        .filter((c) => d.group.has(c.clip.id))
        .map((c) => ({ op: "move_clip", id: c.clip.id, start: Math.max(0, c.clip.start + delta) }));
      api.applyEdit(cmds, tr(`クリップ ${cmds.length} 個を移動`, `Move ${plural(cmds.length, "clip")}`)).catch(() => {});
      return;
    }
    suppressOpen = true;
    setTimeout(() => (suppressOpen = false), 400);
    if (d.mode === "resize") {
      if (d.previewLength !== d.clip.length) {
        api
          .applyEdit(
            [{ op: "resize_clip", id: d.clip.id, length: d.previewLength }],
            tr(
              `${d.clip.name} の長さを ${(d.previewLength / (project.ppq * 4)).toFixed(2)} 小節相当に変更`,
              `Resize ${d.clip.name} to ${(d.previewLength / (project.ppq * 4)).toFixed(2)} bars`,
            ),
          )
          .catch(() => {});
      }
    } else if (d.previewStart !== d.clip.start || d.previewTrackId !== d.trackId) {
      const cmd: Record<string, unknown> = {
        op: "move_clip",
        id: d.clip.id,
        start: d.previewStart,
      };
      if (d.previewTrackId !== d.trackId) cmd.track = d.previewTrackId;
      const destName = project.tracks.find((t) => t.id === d.previewTrackId)?.name ?? "";
      api
        .applyEdit(
          [cmd],
          d.previewTrackId !== d.trackId
            ? tr(`${d.clip.name} を ${destName} へ移動`, `Move ${d.clip.name} to ${destName}`)
            : tr(`${d.clip.name} を移動`, `Move ${d.clip.name}`),
        )
        .catch(() => {});
    }
  }

  // ---- クリップの選択・分割・削除・コピー ----

  let selectedClips = $state<Set<string>>(new Set());

  // ---- トラックの複数選択(まとめて削除・ミュート・ソロ)。クリップの選択とはどちらか一方 ----
  let selectedTracks = $state<Set<string>>(new Set());
  /** Shift+クリックで範囲を選ぶときの起点 */
  let trackAnchor: string | null = null;
  $effect(() => {
    if (selectedClips.size > 0) untrack(() => selectedTracks.size > 0 && (selectedTracks = new Set()));
  });
  $effect(() => {
    // 消えたトラックは選択から外す
    const ids = new Set(project.tracks.map((t) => t.id));
    const cur = untrack(() => selectedTracks);
    if ([...cur].some((id) => !ids.has(id))) selectedTracks = new Set([...cur].filter((id) => ids.has(id)));
  });
  /** 見出しのクリックで選ぶ: そのトラックだけ / Ctrl で足す・外す / Shift で範囲 */
  function onTrackHeadClick(e: MouseEvent, track: Track) {
    if ((e.target as HTMLElement).closest("button, input, select, .grip, .w-grip, .h-grip")) return;
    const ids = project.tracks.map((t) => t.id);
    if (e.shiftKey && trackAnchor && ids.includes(trackAnchor)) {
      const [a, b] = [ids.indexOf(trackAnchor), ids.indexOf(track.id)].sort((x, y) => x - y);
      selectedTracks = new Set(ids.slice(a, b + 1));
    } else if (e.ctrlKey || e.metaKey) {
      const next = new Set(selectedTracks);
      if (next.has(track.id)) next.delete(track.id);
      else next.add(track.id);
      selectedTracks = next;
      trackAnchor = track.id;
    } else {
      selectedTracks = new Set([track.id]);
      trackAnchor = track.id;
    }
    if (selectedTracks.size > 0) selectedClips = new Set();
  }
  /** 操作の対象: 押したトラックが選択の中なら選んだ全部、そうでなければそのトラックだけ */
  const targetsOf = (t: Track): Track[] =>
    selectedTracks.has(t.id) && selectedTracks.size > 1 ? project.tracks.filter((x) => selectedTracks.has(x.id)) : [t];

  function deleteTracks(tracks: Track[]) {
    if (!tracks.length) return;
    const label =
      tracks.length === 1
        ? tr(`${tracks[0].name} を削除`, `Delete ${tracks[0].name}`)
        : tr(`${tracks.length} トラックを削除`, `Delete ${plural(tracks.length, "track")}`);
    api.applyEdit(tracks.map((t) => ({ op: "remove_track", id: t.id })), label).catch(() => {});
    selectedTracks = new Set();
  }

  function allClips(): { track: Track; clip: Clip }[] {
    return project.tracks.flatMap((track) => track.clips.map((clip) => ({ track, clip })));
  }

  // 消えたクリップ(undo・AI の削除)を選択から外す。
  // 貼り付け直後の新しいクリップは、まだプロジェクトに届いていないだけなので残す
  // (「前回あって今回ない」ものだけ外す)
  let knownClipIds = new Set<string>();
  $effect(() => {
    const ids = new Set(allClips().map((c) => c.clip.id));
    const gone = [...selectedClips].filter((id) => knownClipIds.has(id) && !ids.has(id));
    knownClipIds = ids;
    if (gone.length > 0) {
      selectedClips = new Set([...selectedClips].filter((id) => !gone.includes(id)));
    }
  });

  function selectedList(): { track: Track; clip: Clip }[] {
    return allClips().filter((c) => selectedClips.has(c.clip.id));
  }

  function deleteClips(ids: string[]) {
    if (ids.length === 0) return;
    api
      .applyEdit(
        ids.map((id) => ({ op: "remove_clip", id })),
        ids.length === 1 ? tr("クリップを削除", "Delete clip") : tr(`クリップ ${ids.length} 個を削除`, `Delete ${plural(ids.length, "clip")}`),
      )
      .catch(() => {});
    selectedClips = new Set();
  }

  function setLoop(clip: Clip, on: boolean) {
    if (clip.kind !== "midi") return;
    api
      .applyEdit(
        [{ op: "set_clip_loop", id: clip.id, loop_len: on ? clip.length : null }],
        on ? tr(`${clip.name} をループにする`, `Loop ${clip.name}`) : tr(`${clip.name} のループを解除`, `Unloop ${clip.name}`),
      )
      .catch(() => {});
  }

  /// 音声クリップのテンポ追従。ON のときは今のテンポ(クリップ先頭)で録った素材として扱う
  function setFollow(targets: Clip[], on: boolean) {
    const cmds = targets
      .filter((c) => c.kind === "audio")
      .map((c) => ({
        op: "set_clip_stretch",
        id: c.id,
        stretch: on ? { mode: "follow", original_bpm: bpmAt(project, c.start) } : { mode: "none" },
      }));
    if (cmds.length === 0) return;
    const label = on
      ? tr(
          `${cmds.length === 1 ? targets[0].name : `音声クリップ ${cmds.length} 個`} をテンポに追従させる`,
          `Make ${cmds.length === 1 ? targets[0].name : `${cmds.length} audio clips`} follow tempo`,
        )
      : tr("テンポ追従を解除", "Stop following tempo");
    api.applyEdit(cmds, label).catch(() => {});
  }

  /// 素材の元のテンポを自動で検出してテンポ追従にする(時間がかかるのでクリップに「検出中…」を出す)
  let detectingTempo = $state<string | null>(null);
  async function setFollowDetected(targets: Clip[]) {
    if (detectingTempo) return;
    const audio = targets.filter((c) => c.kind === "audio");
    const cmds: Record<string, unknown>[] = [];
    const found: string[] = [];
    const failed: string[] = [];
    try {
      for (const c of audio) {
        detectingTempo = c.id;
        const r = await api.detectClipTempo(c.id);
        if (r.bpm === null) {
          failed.push(c.name);
          continue;
        }
        cmds.push({ op: "set_clip_stretch", id: c.id, stretch: { mode: "follow", original_bpm: r.bpm } });
        found.push(`${c.name}: ${r.bpm} BPM`);
      }
    } catch (e) {
      showError(tr("テンポを検出できませんでした", "Couldn't detect the tempo"), e);
      return;
    } finally {
      detectingTempo = null;
    }
    if (failed.length > 0) {
      showToast(
        "warn",
        tr(
          `テンポを検出できませんでした(拍のはっきりしない音か、短すぎます): ${failed.join("、")}`,
          `Couldn't detect the tempo (the beat is unclear or the audio is too short): ${failed.join(", ")}`,
        ),
      );
    }
    if (cmds.length === 0) return;
    const label =
      cmds.length === 1
        ? tr(`${found[0]} の素材としてテンポに追従させる`, `Follow tempo as ${found[0]} material`)
        : tr(
            `音声クリップ ${cmds.length} 個をテンポに追従させる(元のテンポを検出)`,
            `Make ${cmds.length} audio clips follow tempo (original tempo detected)`,
          );
    api.applyEdit(cmds, label).catch(() => {});
  }

  /// 似た音の CLAP プリセットを探すダイアログの対象クリップ
  let similarFor = $state<Clip | null>(null);

  /// 音声クリップの音に似せた内蔵シンセのトラックを作る(時間がかかるのでクリップに「音色を合わせています…」を出す)
  let matching = $state<string | null>(null);
  async function matchSound(clip: Clip) {
    if (matching || clip.kind !== "audio") return;
    matching = clip.id;
    try {
      const r = await api.matchClipSound(clip.id);
      showToast(
        "ok",
        tr(
          `「${r.track_name}」を作りました(音源: ${r.instrument}${r.reverb ? " + リバーブ" : ""}、近さ: ${r.verdict}、距離 ${r.initial_distance.toFixed(2)} → ${r.distance.toFixed(2)})。\n` +
            "音作りビューでつまみを微調整できます(Ctrl+Z で取り消し)。",
          `Created "${r.track_name}" (instrument: ${r.instrument}${r.reverb ? " + reverb" : ""}, match: ${r.verdict}, distance ${r.initial_distance.toFixed(2)} → ${r.distance.toFixed(2)}).\n` +
            "Fine-tune the knobs in the Inspector (Ctrl+Z to undo).",
        ),
      );
    } catch (e) {
      showError(tr("似た音を作れませんでした", "Couldn't create a similar sound"), e);
    } finally {
      matching = null;
    }
  }

  function expandLoop(clip: Clip) {
    const cmd = expandLoopCommand(clip);
    if (!cmd) return;
    api.applyEdit([cmd], tr(`${clip.name} の繰り返しをノートに展開`, `Expand ${clip.name} loop into notes`)).catch(() => {});
  }

  /// `at`(絶対 tick)で分割。範囲外のクリップは対象外。
  /// ループクリップは繰り返しをノートに展開してから分割する(同じ 1 undo)
  function splitClips(targets: Clip[], at: number) {
    const cmds = targets
      .filter((c) => at > c.start && at < c.start + c.length)
      .flatMap((c) => {
        const expand = expandLoopCommand(c);
        const split = { op: "split_clip", id: c.id, at: Math.round(at), new_id: newClipId() };
        return expand ? [expand, split] : [split];
      });
    if (cmds.length === 0) return false;
    const n = cmds.filter((c) => c.op === "split_clip").length;
    api
      .applyEdit(cmds, n === 1 ? tr("クリップを分割", "Split clip") : tr(`クリップ ${n} 個を分割`, `Split ${plural(n, "clip")}`))
      .catch(() => {});
    return true;
  }

  /// コピー: クリップの中身と、元のトラック・先頭からの相対位置を覚える
  let clipBoard: { trackId: string; offset: number; clip: Clip }[] = [];

  function copyClips(cut: boolean) {
    const list = selectedList();
    if (list.length === 0) return;
    const minStart = Math.min(...list.map((c) => c.clip.start));
    clipBoard = list.map((c) => ({
      trackId: c.track.id,
      offset: c.clip.start - minStart,
      clip: JSON.parse(JSON.stringify(c.clip)) as Clip,
    }));
    if (cut) deleteClips(list.map((c) => c.clip.id));
  }

  /// 貼り付け: 再生ヘッド(1 拍に丸める)を先頭に、元のトラックへ置く
  function pasteClips() {
    if (clipBoard.length === 0) return;
    const at = Math.round(playheadTick / project.ppq) * project.ppq;
    const tracks = new Set(project.tracks.map((t) => t.id));
    const cmds = clipBoard
      .filter((b) => tracks.has(b.trackId))
      .map((b) => ({ op: "add_clip", track: b.trackId, clip: cloneClip(b.clip, at + b.offset) }));
    if (cmds.length === 0) return;
    api.applyEdit(cmds, tr(`クリップ ${cmds.length} 個を貼り付け`, `Paste ${plural(cmds.length, "clip")}`)).catch(() => {});
    selectedClips = new Set(cmds.map((c) => c.clip.id as string));
  }

  /// 複製: 選択範囲の直後に同じ並びで置く
  function duplicateClips() {
    const list = selectedList();
    if (list.length === 0) return;
    const minStart = Math.min(...list.map((c) => c.clip.start));
    const maxEnd = Math.max(...list.map((c) => c.clip.start + c.clip.length));
    const cmds = list.map((c) => ({
      op: "add_clip",
      track: c.track.id,
      clip: cloneClip(c.clip, maxEnd + (c.clip.start - minStart)),
    }));
    api.applyEdit(cmds, tr(`クリップ ${cmds.length} 個を複製`, `Duplicate ${plural(cmds.length, "clip")}`)).catch(() => {});
    selectedClips = new Set(cmds.map((c) => c.clip.id as string));
  }

  // キー操作(ピアノロール表示中・入力中はピアノロール / 入力欄に譲る)
  $effect(() => {
    const onKey = (e: KeyboardEvent) => {
      // 開いているメニューは Esc で閉じる(メニュー内の入力欄にフォーカスがあっても)
      if (e.key === "Escape" && (trackMenu || clipMenu || addMenu || sigMenu)) {
        e.preventDefault();
        trackMenu = null;
        clipMenu = null;
        addMenu = null;
        sigMenu = null;
        return;
      }
      if (shouldYieldKey(e)) return;
      if (pianoRollStore.focus) return;
      const mod = e.ctrlKey || e.metaKey;
      // 選んだトラック: Delete でまとめて削除、Esc で選択を外す
      if (selectedTracks.size > 0 && selectedClips.size === 0) {
        if (e.key === "Delete" || e.key === "Backspace") {
          e.preventDefault();
          deleteTracks(project.tracks.filter((t) => selectedTracks.has(t.id)));
          return;
        }
        if (e.key === "Escape") {
          selectedTracks = new Set();
          return;
        }
      }
      if (mod && e.code === "KeyA") {
        e.preventDefault();
        selectedClips = new Set(allClips().map((c) => c.clip.id));
      } else if (selectedClips.size === 0 && !(mod && e.code === "KeyV")) {
        return;
      } else if (e.key === "Delete" || e.key === "Backspace") {
        e.preventDefault();
        deleteClips([...selectedClips]);
      } else if (mod && e.code === "KeyC") {
        e.preventDefault();
        copyClips(false);
      } else if (mod && e.code === "KeyX") {
        e.preventDefault();
        copyClips(true);
      } else if (mod && e.code === "KeyV") {
        e.preventDefault();
        pasteClips();
      } else if (mod && e.code === "KeyD") {
        e.preventDefault();
        duplicateClips();
      } else if (!mod && !e.altKey && e.code === "KeyS") {
        e.preventDefault();
        splitClips(selectedList().map((c) => c.clip), playheadTick);
      } else if (e.key === "Escape") {
        selectedClips = new Set();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  // 右クリックメニュー
  let clipMenu = $state<{ x: number; y: number; clip: Clip; at: number } | null>(null);

  function onClipContext(e: MouseEvent, clip: Clip) {
    e.preventDefault();
    if (!selectedClips.has(clip.id)) selectedClips = new Set([clip.id]);
    const lane = (e.currentTarget as HTMLElement).closest(".lane") as HTMLElement | null;
    const x = lane ? e.clientX - lane.getBoundingClientRect().left : clip.start * pxPerTick;
    const snap = e.altKey ? 1 : project.ppq;
    const at = Math.round(x / pxPerTick / snap) * snap;
    clipMenu = { x: e.clientX, y: e.clientY, clip, at };
  }

  function menuAction(action: ClipMenuAction) {
    const m = clipMenu;
    clipMenu = null;
    if (!m) return;
    const targets = selectedClips.has(m.clip.id) ? selectedList().map((c) => c.clip) : [m.clip];
    switch (action) {
      case "split":
        splitClips(targets, m.at);
        break;
      case "split-head":
        splitClips(targets, playheadTick);
        break;
      case "dup":
        duplicateClips();
        break;
      case "copy":
        copyClips(false);
        break;
      case "cut":
        copyClips(true);
        break;
      case "delete":
        deleteClips(targets.map((c) => c.id));
        break;
      case "loop-on":
        setLoop(m.clip, true);
        break;
      case "loop-off":
        setLoop(m.clip, false);
        break;
      case "expand":
        expandLoop(m.clip);
        break;
      case "follow-on":
        setFollow(targets, true);
        break;
      case "follow-detect":
        setFollowDetected(targets);
        break;
      case "follow-off":
        setFollow(targets, false);
        break;
      case "sep-builtin":
        separate(m.clip, "builtin");
        break;
      case "sep-demucs":
        separate(m.clip, "demucs");
        break;
      case "match":
        matchSound(m.clip);
        break;
      case "similar":
        similarFor = m.clip;
        break;
    }
  }

  function barLabel(tick: number): string {
    const b = barAtTick(barList, tick);
    const beat = Math.floor((tick - b.tick) / project.ppq) + 1;
    return tr(`${b.index + 1} 小節 ${beat} 拍`, `Bar ${b.index + 1} beat ${beat}`);
  }

  // ---- オートメーションレーンの開閉 ----

  /// 開いているオートメーションレーン(トラック ID → 表示中のパラメータのパス)
  let autoLanes = $state<Record<string, string>>({});

  function toggleAutoLane(trackId: string) {
    if (autoLanes[trackId]) {
      const next = { ...autoLanes };
      delete next[trackId];
      autoLanes = next;
    } else {
      // 描かれているレーンがあればそれを、無ければ音量を開く
      const lanes =
        trackId === MASTER_FOCUS_ID
          ? (project.master.automation ?? [])
          : (project.tracks.find((t) => t.id === trackId)?.automation ?? []);
      const existing = lanes.find((l) => l.points.length > 0)?.target;
      autoLanes = { ...autoLanes, [trackId]: existing ?? "track/volume_db" };
    }
  }

  /// マスターのオートメーションレーン用の擬似トラック(AutomationLaneRow に渡す)
  const masterTrack = $derived<Track>({
    id: MASTER_FOCUS_ID,
    name: tr("マスター", "Master"),
    kind: "audio",
    mute: false,
    solo: false,
    volume_db: project.master.volume_db,
    pan: 0,
    device: null,
    effects: project.master.effects,
    clips: [],
    automation: project.master.automation ?? [],
  });
  const masterLaneCount = $derived(
    (project.master.automation ?? []).filter((l) => l.points.length > 0).length,
  );

  function hasVolumeLane(t: Track): boolean {
    return t.automation.some(
      (l) => l.target === "track/volume_db" && l.points.length > 0,
    );
  }

  // ---- トラックの右クリックメニュー(移動・削除) ----

  let trackMenu = $state<{ trackId: string; index: number; x: number; y: number } | null>(null);

  /// メニューに持たせる位置は、画面の並び(先に動かしている間はずれる)ではなく実際の並びで
  function trackIndex(track: Track): number {
    return project.tracks.findIndex((t) => t.id === track.id);
  }

  function openTrackMenu(e: MouseEvent, track: Track) {
    e.preventDefault();
    trackMenu = { trackId: track.id, index: trackIndex(track), x: e.clientX, y: e.clientY };
  }

  function moveTrack(toIndex: number) {
    const menu = trackMenu;
    trackMenu = null;
    if (!menu || toIndex < 0 || toIndex >= project.tracks.length) return;
    reorderTrack(menu.trackId, toIndex);
  }

  // ---- 並べ替えを先に画面へ(保存の往復を待たずに動かし、animate:flip で滑らかに入れ替える) ----
  /// 先に反映している並び(トラック ID)。実際の並びが追いついたら外す
  let optimisticOrder = $state<string[] | null>(null);
  let optimisticTimer: ReturnType<typeof setTimeout> | undefined;

  const shownTracks = $derived.by((): Track[] => {
    const order = optimisticOrder;
    if (!order) return project.tracks;
    const byId = new Map(project.tracks.map((t) => [t.id, t]));
    const list = order.map((id) => byId.get(id)).filter((t): t is Track => !!t);
    // その間にトラックが足された・消されたときは実際の並びに戻す
    return list.length === project.tracks.length ? list : project.tracks;
  });

  $effect(() => {
    const order = optimisticOrder;
    if (order && project.tracks.map((t) => t.id).join() === order.join()) optimisticOrder = null;
  });

  function reorderTrack(id: string, to: number) {
    const ids = project.tracks.map((t) => t.id);
    const from = ids.indexOf(id);
    if (from < 0 || to === from) return;
    ids.splice(from, 1);
    ids.splice(to, 0, id);
    optimisticOrder = ids;
    clearTimeout(optimisticTimer);
    optimisticTimer = setTimeout(() => (optimisticOrder = null), 3000);
    const name = project.tracks[from]?.name ?? tr("トラック", "Track");
    api
      .applyEdit(
        [{ op: "move_track", id, to_index: to }],
        tr(`${name} を ${to + 1} 番目へ移動`, `Move ${name} to position ${to + 1}`),
      )
      .catch(() => (optimisticOrder = null));
  }

  // ---- トラックの名前・色・複製 ----

  /// 名前を変更中のトラック ID
  let renaming = $state<string | null>(null);

  function startRename(trackId: string) {
    trackMenu = null;
    renaming = trackId;
  }

  function commitRename(track: Track, value: string) {
    renaming = null;
    renameTrack(track, value);
  }

  function setMenuTrackColor(color: string | null) {
    const menu = trackMenu;
    trackMenu = null;
    if (!menu) return;
    setTrackColor(menu.trackId, project.tracks[menu.index], color);
  }

  /// トラックを複製して直後に置く(クリップ・ノート・エフェクトの ID は新しく振る)
  function duplicateTrack() {
    const menu = trackMenu;
    trackMenu = null;
    if (!menu) return;
    const src = project.tracks[menu.index];
    if (!src) return;
    // 複製はバックエンドで(画面が受け取るトラックには CLAP の状態が入っていないため)
    api.duplicateTrack(src.id).catch(() => {});
  }

  /// トラックを音声にする(描き出しに曲の長さの数分の 1 かかる)
  let bouncing = $state<string | null>(null);
  async function bounceTrack() {
    const menu = trackMenu;
    trackMenu = null;
    if (!menu || bouncing) return;
    const track = project.tracks[menu.index];
    bouncing = menu.trackId;
    showToast(
      "ok",
      tr(`「${track?.name ?? "トラック"}」を音声に描き出しています…`, `Rendering "${track?.name ?? "track"}" to audio…`),
    );
    try {
      const r = await api.bounceTrack(menu.trackId);
      showToast(
        "ok",
        tr(
          `「${track?.name}」を音声にしました(${r.seconds.toFixed(1)} 秒。元のトラックはミュート、Ctrl+Z で戻せます)`,
          `Rendered "${track?.name}" to audio (${r.seconds.toFixed(1)} s. The original track is muted; Ctrl+Z to undo)`,
        ),
      );
    } catch (e) {
      showError(tr("音声にできませんでした", "Couldn't render to audio"), e);
    } finally {
      bouncing = null;
    }
  }

  function deleteTrack() {
    const menu = trackMenu;
    trackMenu = null;
    if (!menu) return;
    const track = project.tracks.find((t) => t.id === menu.trackId);
    if (!track) return;
    deleteTracks(targetsOf(track));
  }

  // ---- 音源(デバイス)の選択メニュー ----

  // CLAP プラグイン(外部音源)。一覧は初回だけ探し、以後は使い回す
  let clapList = $state<api.ClapPluginInfo[] | null>(null);
  let clapDirs = $state<string[]>([]);
  let clapLoading = $state(false);
  async function loadClap(rescan = false) {
    if (clapLoading || (clapList && !rescan)) return;
    clapLoading = true;
    try {
      const r = await api.clapPlugins(rescan);
      clapList = r.plugins;
      clapDirs = r.dirs;
    } catch {
      clapList = [];
    } finally {
      clapLoading = false;
    }
  }
  const clapNames = $derived(new Map((clapList ?? []).map((p) => [p.id, p.name])));
  // CLAP 音源のトラックがあれば、見出しにプラグイン名を出すため一覧を読んでおく
  $effect(() => {
    if (!clapList && project.tracks.some((t) => t.device?.type === "clap")) loadClap();
  });

  /// 音源ピッカーを開く(見出しの音源名から。インスペクターの「変更」と同じもの)
  function openPicker(e: MouseEvent, track: Track) {
    e.stopPropagation();
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    instrumentPickerStore.open = { trackId: track.id, x: r.left, y: r.bottom + 4 };
  }

  /// 見出しの ⋯ からトラックのメニューを開く(右クリックと同じもの)
  function openTrackMenuAt(e: MouseEvent, track: Track) {
    e.stopPropagation();
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    trackMenu = { trackId: track.id, index: trackIndex(track), x: r.left, y: r.bottom + 4 };
  }

  function openClapGui(trackId: string) {
    trackMenu = null;
    api.clapOpenGui(trackId).catch((err) => showError(tr("プラグインの画面を開けませんでした", "Couldn't open the plugin window"), err));
  }

  // ---- トラックの並べ替え(見出しのつまみをつかんで上下にドラッグ) ----
  // つかんだトラックはポインターについて動き、ほかのトラックはすき間を空けるように滑る。
  // 離すと、並びを先に画面へ反映して(reorderTrack)、つかんだトラックは今の位置から収まる所へ滑り込む
  let trackDrag = $state<{ id: string; from: number; to: number; dy: number; h: number } | null>(null);

  function onGripDown(e: PointerEvent, track: Track, ti: number) {
    if (e.button !== 0 || !root) return;
    e.preventDefault();
    e.stopPropagation();
    const blocks = [...root.querySelectorAll<HTMLElement>(".track-block")];
    const rects = blocks.map((b) => b.getBoundingClientRect());
    const self = rects[ti];
    if (!self) return;
    const startY = e.clientY;
    trackDrag = { id: track.id, from: ti, to: ti, dy: 0, h: self.height };
    const move = (ev: PointerEvent) => {
      if (!trackDrag) return;
      const dy = ev.clientY - startY;
      // つかんだトラックの中心より上にある(ほかの)トラックの数 = 新しい位置
      const center = self.top + self.height / 2 + dy;
      const to = rects.filter((r, i) => i !== ti && r.top + r.height / 2 < center).length;
      trackDrag = { ...trackDrag, dy, to };
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      const d = trackDrag;
      trackDrag = null;
      if (!d) return;
      if (d.to !== d.from) {
        reorderTrack(d.id, d.to);
      } else if (d.dy !== 0) {
        // 動かさなかったときは元の位置へ滑って戻る
        blocks[ti]?.animate([{ transform: `translateY(${d.dy}px)` }, { transform: "none" }], {
          duration: 160,
          easing: "ease-out",
        });
      }
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  /// ドラッグ中の各トラックのずれ(つかんだトラックはポインターの分、間のトラックはすき間の分)
  function dragShift(ti: number): number {
    const d = trackDrag;
    if (!d) return 0;
    if (ti === d.from) return d.dy;
    if (d.from < d.to && ti > d.from && ti <= d.to) return -d.h;
    if (d.to < d.from && ti >= d.to && ti < d.from) return d.h;
    return 0;
  }

  // ---- トラックの追加(1 つのボタンから種類を選ぶ) ----
  let addMenu = $state<{ x: number; y: number } | null>(null);

  function openAddMenu(e: MouseEvent) {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    addMenu = { x: r.left, y: r.bottom + 4 };
  }

  function addFromMenu(kind: "midi" | "audio" | "bus" | "midi-file") {
    addMenu = null;
    if (kind === "midi-file") importMidiFile();
    else addTrack(project, kind);
  }

  /// マスター音量のドラッグ中の値(離すまで表示と試聴だけ)
  let masterDrag = $state<number | null>(null);

  function onMasterVolume(e: Event) {
    masterDrag = null;
    setMasterVolume(Number((e.currentTarget as HTMLInputElement).value));
  }

  function toggleSolo(t: Track) {
    toggleTrackFlag("solo", t, targetsOf(t));
  }

  // ---- 設計データの状態の印 ----
  function openDesignFor(track: { id: string }, e: MouseEvent) {
    e.stopPropagation();
    const idx = project.tracks.filter((t) => t.kind === "midi").findIndex((t) => t.id === track.id);
    if (idx >= 0) {
      designSel.part = idx;
      designSel.sel = { kind: "part", p: idx };
    }
    viewStore.main = "design";
  }
</script>

<div class="timeline" class:w-hot={wGripHot} bind:this={root} style="--head-w:{HEAD_W}px;--track-h:{defaultTrackH}px;--grid:{gridImage};--grid-w:{Math.max(1, Math.ceil(totalPx))}px">
  <!-- 再生ヘッド -->
  <div class="playhead" class:under-head={playheadPx < scrollX + HEAD_W} style="left:{playheadPx}px"></div>

  <!-- 選択中の小節範囲(チャット指示のマスク) -->
  {#if selection}
    <div
      class="selection-overlay"
      style="left:{HEAD_W + selection.startTick * pxPerTick}px;width:{(selection.endTick -
        selection.startTick) *
        pxPerTick}px"
    ></div>
  {/if}

  <!-- セクションマーカー(曲の構成。AI が set_sections で管理) -->
  <TimelineSections {project} {barList} {pxPerTick} {totalPx} onSelect={setRange} />

  <!-- 小節ルーラー(クリックでシーク、ドラッグで範囲選択) -->
  <div class="ruler-row" style="top:{project.sections && project.sections.length > 0 ? SECTION_ROW_H : 0}px">
    <div class="track-head ruler-head">
      <!-- svelte-ignore a11y_no_static_element_interactions --><span class="w-grip" onpointerenter={() => (wGripHot = true)} onpointerleave={() => (wGripHot = !!document.body.classList.contains("resizing-w"))} onpointerdown={(e) => startLayoutDrag(e, "w")} ondblclick={() => resetLayout("w")} title={tr("ドラッグで見出しの幅を変える(ダブルクリックで元に戻す)", "Drag to change the header width (double-click to reset)")}></span>
      <TimelineZoom headW={HEAD_W} trackH={defaultTrackH} onZoomBy={zoomBy} onFit={zoomToFit} onSetZoom={(z) => setZoom(z)} />
    </div>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="lane seekable"
      style="width:{totalPx}px"
      onpointerdown={onRulerDown}
      onpointermove={onRulerMove}
      onpointerup={onRulerUp}
      onpointercancel={() => {
        stopAutoScroll();
        dragStart = null;
        dragging = false;
      }}
      oncontextmenu={onRulerContext}
      title={tr("クリックで移動、ドラッグで範囲選択、右クリックでその小節から拍子を変更", "Click to move, drag to select a range, right-click to change the time signature from that bar")}
    >
      {#each barList as bar (bar.index)}
        <div class="bar-mark" class:quiet={bar.index % barLabelEvery !== 0} style="left:{bar.tick * pxPerTick}px">
          {#if bar.index % barLabelEvery === 0}{bar.index + 1}{/if}{#if barLabelEvery === 1 && chordAt.get(bar.tick)}<span class="chord-chip">{chordAt.get(bar.tick)}</span>{/if}{#if barLabelEvery === 1 && tempoAt.get(bar.tick)}<span
              class="tempo-chip"
              title={tr("この小節からのテンポ(右クリックで変更・削除)", "Tempo from this bar (right-click to change or delete)")}>♩={fmtBpm(tempoAt.get(bar.tick) ?? 0)}</span
            >{/if}{#if bar.sigChange && barLabelEvery === 1}<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions --><span
              class="sig-chip"
              title={tr("クリックで拍子を編集・削除", "Click to edit or delete the time signature")}
              onpointerdown={(e) => e.stopPropagation()}
              onpointerup={(e) => e.stopPropagation()}
              onclick={(e) => openSigMenu(e, bar.index)}>{meterLabel(bar)}</span
            >{/if}
        </div>
      {/each}
    </div>
  </div>

  <!-- マスター: 曲全体の音量・オートメーション・エフェクト。見つけやすいよう、トラックの一番上に置く
       (以前は一番下で、音量のつまみも短かった) -->
  <div class="track-row master-row">
    <div class="track-head">
      <!-- svelte-ignore a11y_no_static_element_interactions --><span class="w-grip" onpointerenter={() => (wGripHot = true)} onpointerleave={() => (wGripHot = !!document.body.classList.contains("resizing-w"))} onpointerdown={(e) => startLayoutDrag(e, "w")} ondblclick={() => resetLayout("w")} title={tr("ドラッグで見出しの幅を変える(ダブルクリックで元に戻す)", "Drag to change the header width (double-click to reset)")}></span>
      <div class="head-row">
        <span class="track-name master-name" title={tr("曲全体の音量・エフェクト(書き出しにも入る)", "Volume and effects for the whole song (included in exports)")}>{tr("マスター", "Master")}</span>
        <span class="db master-db">{(masterDrag ?? project.master.volume_db).toFixed(1)} dB</span>
        <button
          class="btn sm icon"
          class:on={autoLanes[MASTER_FOCUS_ID] !== undefined}
          onclick={() => toggleAutoLane(MASTER_FOCUS_ID)}
          title={tr(
            `マスターのオートメーション(フェードアウト、マスターのエフェクトの時間変化)${masterLaneCount > 0 ? `。描いてあるレーン ${masterLaneCount} 本` : ""}`,
            `Master automation (fade-outs, master effect changes over time)${masterLaneCount > 0 ? `. ${masterLaneCount} lanes drawn` : ""}`,
          )}
          aria-label={tr("マスターのオートメーション", "Master automation")}><Icon name="spline" /></button
        >
        <button
          class="btn sm icon"
          class:on={soundDesignStore.focus?.trackId === MASTER_FOCUS_ID}
          onclick={() =>
            (soundDesignStore.focus =
              soundDesignStore.focus?.trackId === MASTER_FOCUS_ID ? null : { trackId: MASTER_FOCUS_ID, trackName: tr("マスター", "Master") })}
          title={tr(
            `マスターのエフェクト(曲全体に掛かるコンプ・EQ・リバーブなど)${project.master.effects.length > 0 ? `。${project.master.effects.length} 個` : ""}`,
            `Master effects (compressor, EQ, reverb, etc. on the whole song)${project.master.effects.length > 0 ? `. ${project.master.effects.length} effects` : ""}`,
          )}
          aria-label={tr("マスターのエフェクト", "Master effects")}><Icon name="sliders-horizontal" /></button
        >
      </div>
      <input
        class="vol master-vol"
        type="range"
        min="-40"
        max="6"
        step="0.5"
        value={project.master.volume_db}
        title={tr("マスター音量(曲全体。書き出しにも入る。ダブルクリックで 0 dB)", "Master volume (whole song, included in exports; double-click for 0 dB)")}
        aria-label={tr("マスター音量", "Master volume")}
        oninput={(e) => {
          masterDrag = Number(e.currentTarget.value);
          api.previewEdit([{ op: "set_master_volume", volume_db: masterDrag }]);
        }}
        onchange={onMasterVolume}
        ondblclick={() => {
          masterDrag = null;
          api.applyEdit([{ op: "set_master_volume", volume_db: 0 }], tr("マスター音量を 0.0 dB に変更", "Set master volume to 0.0 dB")).catch(() => {});
        }}
      />
    </div>
    <div class="lane" style="width:{totalPx}px"></div>
  </div>
  {#if autoLanes[MASTER_FOCUS_ID]}
    <AutomationLaneRow
      track={masterTrack}
      target={autoLanes[MASTER_FOCUS_ID]}
      {pxPerTick}
      {totalPx}
      onTarget={(t) => (autoLanes = { ...autoLanes, [MASTER_FOCUS_ID]: t })}
      onClose={() => toggleAutoLane(MASTER_FOCUS_ID)}
    />
  {/if}

  {#if project.tracks.length === 0}
    <div class="empty">
      {tr("トラックがありません。AI に「トラックを追加して」と頼んでみてください。", "No tracks yet. Try asking the AI to \"add a track\".")}
    </div>
  {/if}

  {#each shownTracks as track, ti (track.id)}
    <!-- 1 トラック分(行とオートメーション)をまとめて動かす -->
    <div
      class="track-block"
      class:sliding={trackDrag !== null && trackDrag.id !== track.id}
      class:lifted={trackDrag?.id === track.id}
      style={trackDrag ? `transform:translateY(${dragShift(ti)}px)` : ""}
      animate:flip={{ duration: 180 }}
    >
    <div class="track-row" class:alt={ti % 2 === 1} style="--track-h:{trackHeight(track.id)}px">
      <!-- 見出し: 1 段目 = つかむ所・種類・名前・⋯ / 2 段目 = M・S・音量 / 3 段目 = 音源と固定の 3 つ
           (アーム・オートメーション・インスペクター。無いものは空けて、どのトラックでも同じ位置に) -->
      <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
      <div
        class="track-head"
        class:t-sel={selectedTracks.has(track.id)}
        style={track.color ? `--tc:${track.color}` : ""}
        oncontextmenu={(e) => openTrackMenu(e, track)}
        onclick={(e) => onTrackHeadClick(e, track)}
      >
        <!-- svelte-ignore a11y_no_static_element_interactions --><span class="w-grip" onpointerenter={() => (wGripHot = true)} onpointerleave={() => (wGripHot = !!document.body.classList.contains("resizing-w"))} onpointerdown={(e) => startLayoutDrag(e, "w")} ondblclick={() => resetLayout("w")} title={tr("ドラッグで見出しの幅を変える(ダブルクリックで元に戻す)", "Drag to change the header width (double-click to reset)")}></span>
        <!-- svelte-ignore a11y_no_static_element_interactions --><span class="h-grip" onpointerdown={(e) => startLayoutDrag(e, "h", track.id)} ondblclick={() => resetLayout("h", track.id)} title={tr("ドラッグでこのトラックの高さを変える(ダブルクリックで既定の高さに戻す)", "Drag to change this track's height (double-click to reset)")}></span>
        <div class="head-row">
          <span class="grip" role="button" tabindex="-1" aria-label={tr("並べ替え", "Reorder")} title={tr("つかんで上下にドラッグで並べ替え", "Grab and drag up/down to reorder")} onpointerdown={(e) => onGripDown(e, track, ti)}
            ><Icon name="grip-vertical" size={14} /></span
          >
          <span class="kind-ic" title={kindLabel(track.kind)}><Icon name={KIND_ICON[track.kind]} size={14} /></span>
          {#if renaming === track.id}
            <input
              class="track-name-input"
              value={track.name}
              use:focusSelect
              onkeydown={(e) => {
                if (e.isComposing) return;
                if (e.key === "Enter") commitRename(track, e.currentTarget.value);
                else if (e.key === "Escape") renaming = null;
              }}
              onblur={(e) => commitRename(track, e.currentTarget.value)}
            />
          {:else}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div class="track-name" title={tr(`${track.name}(ダブルクリックで名前を変更)`, `${track.name} (double-click to rename)`)} ondblclick={() => (renaming = track.id)}>
              {track.name}
            </div>
          {/if}
          <button
            class="btn sm icon ghost"
            onclick={(e) => openTrackMenuAt(e, track)}
            title={tr("トラックのメニュー(名前・色・並べ替え・複製・音声にする・削除)", "Track menu (name, color, reorder, duplicate, render to audio, delete)")}
            aria-label={tr("トラックのメニュー", "Track menu")}><Icon name="ellipsis" /></button
          >
        </div>
        <div class="head-row">
          <button class="btn letter" class:m-on={track.mute} onclick={() => toggleMute(track)} title={tr("ミュート", "Mute")} aria-pressed={track.mute}
            >M</button
          >
          <button class="btn letter" class:s-on={track.solo} onclick={() => toggleSolo(track)} title={tr("ソロ", "Solo")} aria-pressed={track.solo}
            >S</button
          >
          <input
            class="vol"
            type="range"
            min="-40"
            max="6"
            step="0.5"
            value={track.volume_db}
            disabled={hasVolumeLane(track)}
            title={hasVolumeLane(track)
              ? tr("音量オートメーション使用中(フェーダーより優先されます)", "Volume automation in use (overrides the fader)")
              : tr("音量", "Volume")}
            aria-label={tr("音量", "Volume")}
            onchange={(e) => setVolume(track, e)}
          />
          <span class="db" title={tr("音量(dB)", "Volume (dB)")}>{volumeText(track)}</span>
        </div>
        <div class="head-row">
          {#if track.kind === "bus"}
            <span class="dev plain" title={tr("インスペクターの「送り」で、各トラックからこのバスへ送る量を決めます", "Set how much each track sends to this bus with Sends in the Inspector")}
              >{tr("受けている:", "Receiving:")} {sendersOf(project, track)}{tr(" 本", "")}</span
            >
          {:else if track.kind === "audio"}
            <span class="dev plain">{tr("音声", "Audio")}</span>
          {:else}
            <button
              class="dev"
              onclick={(e) => openPicker(e, track)}
              title={track.device
                ? tr("クリックで音源を変える", "Click to change the instrument")
                : tr("音源未設定(既定の subtractive で発音)。クリックで選ぶ", "No instrument set (plays with the default subtractive). Click to choose")}
            >
              <Icon name={deviceIcon(track.device)} size={12} /><span>{deviceName(track.device, clapNames)}</span><Icon
                name="chevron-down"
                size={12}
              />
            </button>
          {/if}
          {#if track.kind === "midi"}
            <button
              class="btn sm icon"
              class:on={midiArmStore.trackId === track.id}
              class:arm={midiArmStore.trackId === track.id}
              onclick={() => (midiArmStore.trackId = midiArmStore.trackId === track.id ? null : track.id)}
              title={tr("MIDI キーボードでこのトラックを弾く(ON の間は録音が MIDI 録音になります)", "Play this track from a MIDI keyboard (while on, recording becomes MIDI recording)")}
              aria-label={tr("MIDI キーボードで弾く", "Play from MIDI keyboard")}
              aria-pressed={midiArmStore.trackId === track.id}><Icon name="keyboard-music" /></button
            >
          {:else}
            <span class="slot"></span>
          {/if}
          <button
            class="btn sm icon"
            class:on={autoLanes[track.id] !== undefined}
            onclick={() => toggleAutoLane(track.id)}
            title={tr("オートメーション(音量・パン・つまみを時間で動かす)", "Automation (move volume, pan and knobs over time)")}
            aria-label={tr("オートメーション", "Automation")}
            aria-pressed={autoLanes[track.id] !== undefined}><Icon name="spline" /></button
          >
          <button
            class="btn sm icon"
            class:on={soundDesignStore.focus?.trackId === track.id}
            onclick={() =>
              (soundDesignStore.focus =
                soundDesignStore.focus?.trackId === track.id ? null : { trackId: track.id, trackName: track.name })}
            title={tr("インスペクター(音源・エフェクト・送り)", "Inspector (instrument, effects, sends)")}
            aria-label={tr("インスペクター", "Inspector")}
            aria-pressed={soundDesignStore.focus?.trackId === track.id}><Icon name="sliders-horizontal" /></button
          >
        </div>
      </div>
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="lane"
        class:drop-target={clipDrag?.moved &&
          clipDrag.previewTrackId === track.id &&
          clipDrag.previewTrackId !== clipDrag.trackId}
        data-track-id={track.id}
        style="width:{totalPx}px"
        ondblclick={(e) => onLaneDblClick(e, track)}
        onpointerdown={(e) => {
          if (!(e.target as HTMLElement).closest(".clip") && !(e.ctrlKey || e.shiftKey)) {
            selectedClips = new Set();
          }
        }}
        title={track.kind === "bus"
          ? tr("バス: 他のトラックのセンドを受けて、エフェクト → 音量/パン → マスターへ(クリップは置けません)", "Bus: receives sends from other tracks, then effects → volume/pan → master (no clips)")
          : track.kind === "audio"
          ? tr("ダブルクリックで音声ファイル(WAV / MP3 等)をその小節に配置(録音はヘッダーの録音ボタン)", "Double-click to place an audio file (WAV / MP3, etc.) at that bar (record with the record button in the header)")
          : track.clips.length === 0
            ? tr("ダブルクリックでクリップを作成してピアノロールを開く", "Double-click to create a clip and open the piano roll")
            : ""}
      >
        {#each track.clips as clip (clip.id)}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="clip {clip.kind}"
            class:dragging={clipDrag?.moved &&
              (clipDrag.clip.id === clip.id || (clipDrag.mode === "move" && clipDrag.group.has(clip.id)))}
            class:selected={selectedClips.has(clip.id)}
            class:ai-changed={aiHighlight.clips.has(clip.id)}
            style={clipStyle(clip)}
            title={tr(
              `${clip.name} (${clip.id})${clip.kind === "midi" ? " — ダブルクリックでピアノロール" : ""} / クリックで選択(Ctrl・Shift で複数)/ ドラッグで移動(Alt でスナップ解除)/ 右端で長さ変更 / 右クリックで分割・複製・削除 / S: 再生ヘッドで分割、Delete: 削除、Ctrl+C/X/V/D`,
              `${clip.name} (${clip.id})${clip.kind === "midi" ? " — double-click for the piano roll" : ""} / click to select (Ctrl/Shift for multiple) / drag to move (Alt to disable snap) / right edge to resize / right-click to split, duplicate or delete / S: split at playhead, Delete: delete, Ctrl+C/X/V/D`,
            )}
            ondblclick={(e) => openPianoRoll(track, clip, e)}
            oncontextmenu={(e) => onClipContext(e, clip)}
            onpointerdown={(e) => onClipDown(e, track, clip)}
            onpointermove={onClipDragMove}
            onpointerup={onClipUp}
            onpointercancel={() => (clipDrag = null)}
          >
            <span class="clip-name"
              >{#if clip.kind === "audio"}<Icon name="audio-lines" size={12} />{:else if clip.loop && clip.loop_len}<span
                  title={tr("ループのクリップ", "Loop clip")}><Icon name="infinity" size={12} /></span
                >{/if}{clip.name}{#if followBpm(clip) !== null}<span class="follow" title={tr("テンポに追従中(元の素材の BPM)", "Following tempo (original BPM of the source)")}
                  ><Icon name="move-horizontal" size={12} />{followBpm(clip)}</span
                >{/if}
              {#if designStore.clips[clip.id]}
                {@const st = designStore.clips[clip.id]}
                <!-- 設計データの状態の印(計画が先に進んだ・手で直した小節・固定の音)。押すと設計画面のそのパートへ -->
                <button
                  class="clip-marks"
                  type="button"
                  title={clipMarkTitle(st)}
                  aria-label={clipMarkTitle(st)}
                  onpointerdown={(e) => e.stopPropagation()}
                  ondblclick={(e) => e.stopPropagation()}
                  onclick={(e) => openDesignFor(track, e)}
                >
                  {#if st.plan === "ahead"}<span class="mk ahead"><Icon name="refresh-cw" size={10} /></span>{/if}
                  {#if st.edited_bars?.length}<span class="mk edited"><Icon name="pencil" size={10} />{st.edited_bars.reduce((n, [a, b]) => n + b - a + 1, 0)}</span>{/if}
                  {#if st.locked_notes}<span class="mk">🔒{st.locked_notes}</span>{/if}
                  {#if st.plan === "in_sync" && !st.edited_bars?.length && !st.locked_notes}<span class="mk sync"><Icon name="check" size={10} /></span>{/if}
                </button>
              {/if}</span
            >
            {#if clip.kind === "midi"}
              <ClipPreview {clip} widthPx={clip.length * pxPerTick} />
            {:else}
              <AudioClipPreview
                clipId={clip.id}
                start={clip.start}
                length={clip.length}
                widthPx={clip.length * pxPerTick}
                variant={`${clip.gain_db ?? 0}:${followBpm(clip) ?? ""}`}
              />
              <AudioClipHandles
                {project}
                {clip}
                {pxPerTick}
                {transcribing}
                onTranscribe={(mode) => transcribe(track, clip, mode)}
              />
            {/if}
            {#if separating === clip.id}
              <div class="clip-busy">{tr("パートに分離中…", "Separating parts…")}</div>
            {:else if detectingTempo === clip.id}
              <div class="clip-busy">{tr("テンポを検出中…", "Detecting tempo…")}</div>
            {:else if matching === clip.id}
              <div class="clip-busy">{tr("音色を合わせています…", "Matching the sound…")}</div>
            {/if}
            <div class="clip-resize"></div>
          </div>
        {/each}
      </div>
    </div>
    {#if autoLanes[track.id]}
      <AutomationLaneRow
        {track}
        target={autoLanes[track.id]}
        {pxPerTick}
        {totalPx}
        onTarget={(t) => (autoLanes = { ...autoLanes, [track.id]: t })}
        onClose={() => toggleAutoLane(track.id)}
      />
    {/if}
    </div>
  {/each}

  {#if sigMenu}
    <RulerMenu
      {project}
      {barList}
      bind:menu={sigMenu}
      bind:markerName
      onClose={() => (sigMenu = null)}
      onPutMarker={putMarker}
      onRemoveMarker={removeMarkerAt}
    />
  {/if}

  {#if similarFor}
    <SimilarPresetDialog {project} clip={similarFor} onClose={() => (similarFor = null)} />
  {/if}

  {#if clipMenu}
    <ClipMenu
      {project}
      x={clipMenu.x}
      y={clipMenu.y}
      clip={clipMenu.clip}
      atLabel={barLabel(clipMenu.at)}
      selectedCount={selectedClips.size}
      detectingTempo={detectingTempo !== null}
      separating={separating !== null}
      matching={matching !== null}
      onAction={menuAction}
      onClose={() => (clipMenu = null)}
    />
  {/if}

  {#if trackMenu}
    <TrackMenu
      {project}
      x={trackMenu.x}
      y={trackMenu.y}
      index={trackMenu.index}
      bouncing={!!bouncing}
      onRename={() => startRename(trackMenu!.trackId)}
      onColor={setMenuTrackColor}
      onMove={moveTrack}
      onDuplicate={duplicateTrack}
      onBounce={bounceTrack}
      onOpenGui={() => openClapGui(trackMenu!.trackId)}
      onDelete={deleteTrack}
      onClose={() => (trackMenu = null)}
      deleteCount={selectedTracks.has(trackMenu.trackId) ? selectedTracks.size : 1}
    />
  {/if}

  {#if addMenu}
    <AddTrackMenu x={addMenu.x} y={addMenu.y} onPick={addFromMenu} onClose={() => (addMenu = null)} />
  {/if}


  <div class="add-track-row">
    <button class="btn add-track" onclick={openAddMenu} title={tr("トラックを追加(MIDI・音声・バス・MIDI ファイル)", "Add track (MIDI, audio, bus, MIDI file)")}
      ><Icon name="plus" />{tr("トラックを追加", "Add track")}<Icon name="chevron-down" size={14} /></button
    >
  </div>
</div>

<style>
  .timeline {
    min-width: max-content;
    position: relative;
  }

  .playhead {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 1px;
    background: var(--accent);
    box-shadow: 0 0 4px var(--accent);
    z-index: 3;
    pointer-events: none;
  }

  .playhead.under-head {
    display: none;
  }

  .selection-overlay {
    position: absolute;
    top: 0;
    bottom: 0;
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    border-left: 1px solid var(--accent-dim);
    border-right: 1px solid var(--accent-dim);
    z-index: 1;
    pointer-events: none;
  }

  .lane.seekable {
    cursor: pointer;
  }

  .bar-mark {
    pointer-events: none;
  }

  .tempo-chip {
    margin-left: 5px;
    font-size: 10px;
    color: var(--accent);
  }

  /* 縮小して番号を間引いた小節は、区切りの線も薄く */
  .bar-mark.quiet {
    border-left-color: color-mix(in srgb, var(--border) 45%, transparent);
  }

  /* 小節のコード(推定)。小節番号の横に小さく */
  .chord-chip {
    margin-left: 5px;
    font-size: 10px;
    color: var(--human);
    opacity: 0.9;
  }

  .ruler-row,
  .track-row {
    display: flex;
    border-bottom: 1px solid var(--border);
  }

  .track-name-input {
    flex: 1;
    min-width: 0;
    font: inherit;
    font-weight: 600;
    padding: 0 4px;
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--accent-dim);
    border-radius: 3px;
  }

  .track-head {
    width: var(--head-w, 200px);
    flex-shrink: 0;
    padding: 5px 6px 5px 10px;
    background: var(--bg-panel);
    border-right: 1px solid var(--border);
    position: sticky;
    left: 0;
    /* 横にスクロールしたとき、再生ヘッド(3)やクリップの中の印(〜4)を見出しの下に隠す */
    z-index: 5;
  }

  /* 選んだトラック(まとめて削除・ミュート・ソロの対象) */
  .track-head.t-sel {
    background: color-mix(in srgb, var(--accent) 16%, var(--bg-panel));
    box-shadow: inset 3px 0 0 var(--accent);
  }

  .head-row {
    display: flex;
    align-items: center;
    gap: 4px;
    height: 20px;
  }

  /* 見出しの 3 段(72px の行に 20px × 3) */
  .track-row:not(.master-row) > .track-head {
    display: flex;
    flex-direction: column;
    justify-content: space-between;
    border-bottom: 1px solid var(--border);
  }

  /* トラックの色は左端の帯で(名前の文字色にはしない) */
  .track-row > .track-head::before {
    content: "";
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    width: 4px;
    background: var(--tc, transparent);
  }

  .grip {
    display: inline-flex;
    margin-left: -4px;
    color: var(--text-faint);
    cursor: grab;
    touch-action: none;
  }

  .grip:hover {
    color: var(--text);
  }

  .kind-ic {
    display: inline-flex;
    color: var(--text-dim);
  }

  /* 並べ替え: つかんだトラックは浮かせてポインターに付ける。ほかはすき間を空けるように滑る */
  .track-block {
    position: relative;
  }

  .track-block.sliding {
    transition: transform 0.16s ease;
  }

  .track-block.lifted {
    z-index: 6;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.55);
    opacity: 0.96;
  }

  .track-block.lifted .track-head {
    background: var(--bg-raised);
  }

  .letter {
    width: 20px;
    height: 18px;
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

  .btn.arm.on {
    background: color-mix(in srgb, var(--arm) 22%, var(--bg-panel));
    border-color: var(--arm);
    color: var(--danger-text);
  }

  .slot {
    width: 22px;
    flex-shrink: 0;
  }

  .vol {
    flex: 1;
    min-width: 0;
    accent-color: var(--accent);
    height: 14px;
  }

  .db {
    font-size: 10px;
    font-family: var(--mono);
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
    width: 30px;
    text-align: right;
    flex-shrink: 0;
    white-space: nowrap;
  }

  .ruler-head {
    padding: 0;
    height: 24px;
  }

  /* ルーラーは、縦にスクロールしても上端(区間の帯の下)に残す(トラックが多いとシーク・範囲選択・
     小節番号の確認ができなくなっていた)。top は区間の帯の高さ(マークアップで指定) */
  /* 縦にスクロールしたとき、トラックの見出し(5)や持ち上げたトラック(6)より上に残す */
  .ruler-row {
    position: sticky;
    z-index: 7;
  }

  .ruler-row .track-head {
    z-index: 5;
  }

  .ruler-row .lane {
    height: 24px;
    position: relative;
    background: var(--bg-panel);
  }

  .bar-mark {
    position: absolute;
    top: 0;
    bottom: 0;
    border-left: 1px solid var(--border);
    padding-left: 4px;
    font-size: 11px;
    color: var(--text-dim);
    line-height: 24px;
    white-space: nowrap;
  }

  .sig-chip {
    margin-left: 4px;
    padding: 0 4px;
    border-radius: 3px;
    font-size: 9px;
    background: color-mix(in srgb, var(--accent) 25%, transparent);
    color: var(--accent);
  }

  .track-row {
    height: var(--track-h, 72px);
  }

  /* 低くしたときは、見出しの下の段を隠す(はみ出させない) */
  .track-row > .track-head {
    overflow: hidden;
  }

  .track-row .lane {
    position: relative;
    background: var(--grid) 0 0 / var(--grid-w) 100% no-repeat, var(--bg-lane);
  }

  .track-row.master-row .lane {
    background: var(--bg-lane);
  }

  .track-row.master-row {
    height: 56px;
    border-bottom: 2px solid var(--border-strong);
  }

  .master-row .track-head {
    padding: 6px 8px 6px 10px;
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 5px;
  }

  .master-db {
    width: auto;
    margin-right: auto;
  }

  /* マスター音量は見出しの幅いっぱい(以前は名前と横並びで短く、触りにくかった) */
  .vol.master-vol {
    flex: none;
    width: 100%;
    height: 16px;
    margin: 0;
  }

  /* 見出しの右の端(幅)と下の端(高さ)のつかむ所。乗せると色が付く */
  .w-grip,
  .h-grip {
    position: absolute;
    z-index: 3;
  }

  .w-grip {
    top: 0;
    bottom: 0;
    right: -1px;
    width: 6px;
    cursor: col-resize;
  }

  .h-grip {
    left: 0;
    right: 6px;
    bottom: -1px;
    height: 6px;
    cursor: row-resize;
  }

  .timeline.w-hot .w-grip,
  .h-grip:hover {
    background: color-mix(in srgb, var(--accent) 45%, transparent);
  }

  :global(body.resizing-w),
  :global(body.resizing-w *) {
    cursor: col-resize !important;
    user-select: none;
  }

  :global(body.resizing-h),
  :global(body.resizing-h *) {
    cursor: row-resize !important;
    user-select: none;
  }

  .track-name.master-name {
    flex: 0 0 auto;
    margin-right: 4px;
  }

  .track-row.alt .lane {
    background: var(--bg-lane-alt);
  }

  .track-name {
    flex: 1;
    min-width: 0;
    font-weight: 600;
    font-size: 13px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* 音源の名前(押すと音源ピッカー) */
  .dev {
    flex: 1;
    min-width: 0;
    height: 20px;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 0 5px;
    border-radius: var(--r-sm);
    background: var(--bg-raised);
    border: 1px solid var(--border);
    font-size: var(--fs-xs);
    color: var(--text-dim);
    white-space: nowrap;
    overflow: hidden;
  }

  .dev span {
    overflow: hidden;
    text-overflow: ellipsis;
    flex: 1;
    min-width: 0;
    text-align: left;
  }

  .dev:hover:not(.plain) {
    color: var(--text);
  }

  .dev.plain {
    background: none;
    border-color: transparent;
    padding: 0 2px;
  }

  /* 直前のチャットのターンで AI が変えたクリップ */
  .clip.ai-changed {
    box-shadow:
      0 0 0 2px var(--ai),
      0 0 8px var(--ai);
  }

  .clip {
    position: absolute;
    top: 6px;
    bottom: 6px;
    border-radius: 5px;
    overflow: hidden;
    border: 1px solid rgba(255, 255, 255, 0.25);
    padding: 2px 6px;
    cursor: grab;
    touch-action: none;
    user-select: none;
  }

  .clip.selected {
    outline: 2px solid var(--accent);
    outline-offset: -1px;
  }

  /* 小節番号(.bar-mark)はクリックを素通しするが、拍子チップは押せるようにする */
  .sig-chip {
    cursor: pointer;
    pointer-events: auto;
  }

  .clip.dragging {
    opacity: 0.65;
    cursor: grabbing;
    z-index: 2;
  }

  .clip-resize {
    position: absolute;
    top: 0;
    right: 0;
    bottom: 0;
    width: 8px;
    cursor: ew-resize;
    z-index: 2;
  }

  .clip-busy {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 11px;
    color: #fff;
    background: rgba(0, 0, 0, 0.45);
    z-index: 4;
    pointer-events: none;
  }

  .lane.drop-target {
    outline: 2px dashed var(--accent, #7aa2f7);
    outline-offset: -2px;
  }

  .clip.midi {
    background: color-mix(in srgb, var(--clip-midi) 45%, var(--bg));
  }

  .clip.audio {
    background: color-mix(in srgb, var(--clip-audio) 45%, var(--bg));
  }

  .clip-marks {
    display: inline-flex;
    margin-left: 6px;
    vertical-align: middle;
    gap: 2px;
    padding: 0 3px;
    border: 0;
    border-radius: var(--r-sm);
    background: color-mix(in srgb, var(--bg) 70%, transparent);
    font-size: 10px;
    line-height: 14px;
    color: var(--text);
    cursor: pointer;
  }
  .clip-marks .mk {
    display: inline-flex;
    align-items: center;
    gap: 1px;
  }
  .clip-marks .ahead {
    color: var(--accent);
  }
  .clip-marks .edited {
    color: var(--human);
  }
  .clip-marks .sync {
    color: var(--text-dim);
  }
  .clip-name {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 11px;
    white-space: nowrap;
    position: relative;
    z-index: 1;
  }

  .follow {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    opacity: 0.85;
  }

  .empty {
    padding: 40px;
    color: var(--text-dim);
  }

  .add-track-row {
    padding: 8px;
    position: sticky;
    left: 0;
    width: var(--head-w, 200px);
  }

  .add-track {
    width: 100%;
    background: none;
    border-style: dashed;
    color: var(--text-dim);
  }

</style>
