<script lang="ts">
  import { tick } from "svelte";
  import { open as pickFile } from "@tauri-apps/plugin-dialog";
  import * as api from "./api";
  import { shouldYieldKey } from "./keys";
  import { keepInView } from "./menu";
  import { aiHighlight } from "./aiHighlight.svelte";
  import { harmonyStore } from "./harmony.svelte";
  import { showError, showToast } from "./toast.svelte";
  import AutomationLaneRow from "./AutomationLaneRow.svelte";
  import { barAtTick, barsEndTick, buildBars } from "./barMap";
  import AudioClipPreview from "./AudioClipPreview.svelte";
  import ClipPreview from "./ClipPreview.svelte";
  import SimilarPresetDialog from "./SimilarPresetDialog.svelte";
  import { newClipId, newFxId, newNoteId, newTrackId } from "./ids";
  import {
    instrumentPickerStore,
    MASTER_FOCUS_ID,
    midiArmStore,
    pianoRollStore,
    selectionStore,
    soundDesignStore,
    saveTimelineLayout,
    TIMELINE_HEAD_W,
    TIMELINE_TRACK_H,
    timelineLayout,
    timelineZoom,
    TIMELINE_ZOOM_MAX,
    TIMELINE_ZOOM_MIN,
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
  const HEAD_W = $derived(timelineLayout.headW);
  let layoutMenu = $state(false);

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
    const lane = e.currentTarget as HTMLElement;
    const x = e.clientX - lane.getBoundingClientRect().left;
    return barAtTick(barList, Math.max(0, x / pxPerTick)).index;
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
    }
  }

  function onRulerUp(e: PointerEvent) {
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

  let sigMenu = $state<{ x: number; y: number; barIndex: number; num: string; den: string; bpm: string } | null>(
    null,
  );
  const DENS = [1, 2, 4, 8, 16, 32];

  // ---- セクションマーカー(曲の構成)。以前は AI の set_sections でしか置けなかった ----

  interface Marker {
    tick: number;
    name: string;
  }

  const MARKER_PRESETS = ["intro", "Aメロ", "Bメロ", "サビ", "間奏", "outro"];

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
  let markerName = $state("");
  /// ルーラーの右クリックメニューを開いた小節にあるマーカー
  const markerHere = $derived(
    sigMenu ? (project.sections ?? []).find((m) => m.tick === barList[sigMenu!.barIndex]?.tick) : undefined,
  );

  function commitSections(list: Marker[], label: string) {
    const sorted = [...list].sort((a, b) => a.tick - b.tick);
    api.applyEdit([{ op: "set_sections", sections: sorted }], label).catch(() => {});
  }

  function markers(): Marker[] {
    return (project.sections ?? []).map((m) => ({ tick: m.tick, name: m.name }));
  }

  /// 小節の頭にマーカーを置く(既にあれば名前を変える)
  function putMarker(barIndex: number, name: string) {
    const n = name.trim();
    if (!n) return;
    const tick = barList[Math.min(barIndex, barList.length - 1)].tick;
    const list = markers();
    const hit = list.find((m) => m.tick === tick);
    if (hit) hit.name = n;
    else list.push({ tick, name: n });
    sigMenu = null;
    markerName = "";
    commitSections(list, hit ? `マーカーの名前を「${n}」に変更` : `${barIndex + 1} 小節目にマーカー「${n}」を追加`);
  }

  function removeMarker(tick: number) {
    const list = markers();
    const hit = list.find((m) => m.tick === tick);
    if (!hit) return;
    sigMenu = null;
    commitSections(
      list.filter((m) => m.tick !== tick),
      `マーカー「${hit.name}」を削除`,
    );
  }

  /// 名前を変更中のマーカーの tick
  let renamingMarker = $state<number | null>(null);

  function renameMarker(tick: number, name: string) {
    renamingMarker = null;
    const list = markers();
    const hit = list.find((m) => m.tick === tick);
    const n = name.trim();
    if (!hit || !n || n === hit.name) return;
    const old = hit.name;
    hit.name = n;
    commitSections(list, `マーカーの名前を「${old}」から「${n}」に変更`);
  }

  /// マーカーのドラッグ(移動)。動かさずに離したら、その区間を範囲選択にする
  let markerDrag = $state<{ tick: number; x0: number; to: number; moved: boolean } | null>(null);

  function laneBar(e: PointerEvent): number {
    const lane = (e.currentTarget as HTMLElement).parentElement!;
    const x = e.clientX - lane.getBoundingClientRect().left;
    return barAtTick(barList, Math.max(0, x / pxPerTick)).index;
  }

  function onMarkerDown(e: PointerEvent, m: Marker) {
    if (e.button !== 0 || renamingMarker !== null) return;
    e.stopPropagation();
    markerDrag = { tick: m.tick, x0: e.clientX, to: m.tick, moved: false };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }

  function onMarkerMove(e: PointerEvent) {
    const d = markerDrag;
    if (!d) return;
    if (!d.moved && Math.abs(e.clientX - d.x0) < 4) return;
    d.moved = true;
    d.to = barList[laneBar(e)].tick;
  }

  function onMarkerUp(e: PointerEvent, m: Marker, next: number | undefined) {
    const d = markerDrag;
    markerDrag = null;
    if (!d) return;
    if (!d.moved) {
      // クリック: この区間(次のマーカーの手前まで)を範囲選択 = AI への指示の対象
      const a = barAtTick(barList, m.tick).index;
      const endTick = next ?? project.tracks.reduce((mx, t) => Math.max(mx, ...t.clips.map((c) => c.start + c.length)), m.tick + 1);
      const b = barAtTick(barList, Math.max(m.tick, endTick - 1)).index;
      setRange(a, b);
      return;
    }
    if (d.to === m.tick) return;
    const list = markers();
    if (list.some((x) => x.tick === d.to)) return; // 別のマーカーと重なる所には置かない
    const hit = list.find((x) => x.tick === m.tick);
    if (!hit) return;
    hit.tick = d.to;
    commitSections(list, `マーカー「${m.name}」を ${barAtTick(barList, d.to).index + 1} 小節目へ移動`);
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
      bpm: String(bpmAt(bar.tick)),
    };
  }

  // ---- 曲の途中のテンポ変更(ルーラーの右クリック。小節の頭から) ----

  /// 小節頭 tick → その小節から始まるテンポ(先頭以外。ルーラーに札を出す)
  const tempoAt = $derived(new Map(project.tempo_map.filter((e) => e.tick > 0).map((e) => [e.tick, e.bpm])));

  function applyTempo() {
    const m = sigMenu;
    if (!m) return;
    const bpm = Math.round(Number(m.bpm) * 100) / 100;
    if (!(bpm >= 20 && bpm <= 300)) return;
    const bar = barList[m.barIndex];
    // 同じ位置の変更を置き換え、直前と同じテンポになる変更は取り除く(先頭は必ず残す)
    const sorted = [...project.tempo_map.filter((e) => e.tick !== bar.tick), { tick: bar.tick, bpm }].sort(
      (a, b) => a.tick - b.tick,
    );
    const events: { tick: number; bpm: number }[] = [];
    for (const e of sorted) {
      if (events.length > 0 && events[events.length - 1].bpm === e.bpm) continue;
      events.push(e);
    }
    if (events.length === 0 || events[0].tick !== 0) events.unshift({ tick: 0, bpm: project.tempo_map[0]?.bpm ?? 120 });
    sigMenu = null;
    api
      .applyEdit([{ op: "set_tempo", events }], `${bar.index + 1} 小節目からテンポを ${bpm} に変更`)
      .catch(() => {});
  }

  function removeTempo() {
    const m = sigMenu;
    if (!m) return;
    const bar = barList[m.barIndex];
    sigMenu = null;
    const events = project.tempo_map.filter((e) => e.tick !== bar.tick);
    api
      .applyEdit([{ op: "set_tempo", events }], `${bar.index + 1} 小節目のテンポ変更を削除`)
      .catch(() => {});
  }

  function onRulerContext(e: MouseEvent) {
    const lane = e.currentTarget as HTMLElement;
    const x = e.clientX - lane.getBoundingClientRect().left;
    openSigMenu(e, barAtTick(barList, Math.max(0, x / pxPerTick)).index);
  }

  /// 拍子イベント列を整える: tick 順に並べ、直前と同じ拍子の変更は取り除く
  function normalizeSigs(events: { tick: number; num: number; den: number }[]) {
    const sorted = [...events].sort((a, b) => a.tick - b.tick);
    const out: typeof sorted = [];
    for (const ev of sorted) {
      const prev = out[out.length - 1];
      if (prev && prev.num === ev.num && prev.den === ev.den) continue;
      out.push(ev);
    }
    if (out.length === 0 || out[0].tick !== 0) out.unshift({ tick: 0, num: 4, den: 4 });
    return out;
  }

  function applySig() {
    const m = sigMenu;
    if (!m) return;
    const num = Math.round(Number(m.num));
    const den = Number(m.den);
    if (!(num >= 1 && num <= 32) || !DENS.includes(den)) return;
    const bar = barList[m.barIndex];
    const events = normalizeSigs([
      ...project.time_sig_map.filter((e) => e.tick !== bar.tick),
      { tick: bar.tick, num, den },
    ]);
    sigMenu = null;
    api
      .applyEdit(
        [{ op: "set_time_sig", events }],
        `${bar.index + 1} 小節目から拍子を ${num}/${den} に変更`,
      )
      .catch(() => {});
  }

  function removeSig() {
    const m = sigMenu;
    if (!m) return;
    const bar = barList[m.barIndex];
    sigMenu = null;
    const events = normalizeSigs(project.time_sig_map.filter((e) => e.tick !== bar.tick));
    api
      .applyEdit([{ op: "set_time_sig", events }], `${bar.index + 1} 小節目の拍子変更を削除`)
      .catch(() => {});
  }

  // ---- トラック操作(Command API 経由、author: human) ----

  function setVolume(t: Track, e: Event) {
    const v = Number((e.currentTarget as HTMLInputElement).value);
    api
      .applyEdit(
        [{ op: "set_track_prop", id: t.id, prop: "volume_db", value: v }],
        `${t.name} の音量を ${v.toFixed(1)} dB に変更`,
      )
      .catch(() => {});
  }

  function toggleMute(t: Track) {
    api
      .applyEdit(
        [{ op: "set_track_prop", id: t.id, prop: "mute", value: !t.mute }],
        `${t.name} を${t.mute ? "ミュート解除" : "ミュート"}`,
      )
      .catch(() => {});
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
      showError("譜起こしできませんでした", e);
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
      showError("パートに分けられませんでした", e);
    } finally {
      separating = null;
    }
  }

  /// 音声トラックの空きレーン: 音声ファイルを選んでその小節に音声クリップとして置く
  async function importAudioAt(track: Track, startTick: number) {
    const file = await pickFile({
      title: "音声ファイルをクリップとして配置",
      filters: [{ name: "音声(WAV / MP3 / FLAC / OGG / M4A)", extensions: ["wav", "mp3", "flac", "ogg", "m4a", "aac"] }],
    });
    if (typeof file !== "string") return;
    await api.importAudioClip(track.id, file, startTick).catch((e) => showError("取り込めませんでした", e));
  }

  /// MIDI ファイル(.mid)を読み込む: パートごとに新しいトラックを足す(1 回の undo で戻る)
  async function importMidiFile() {
    const file = await pickFile({ title: "MIDI ファイルを読み込む", filters: [{ name: "MIDI", extensions: ["mid", "midi"] }] });
    if (typeof file !== "string") return;
    try {
      const r = await api.importMidi({ path: file });
      showToast("ok", `MIDI を読み込みました: トラック ${r.tracks} 本・ノート ${r.notes} 個${r.tempo_set ? "(テンポと拍子も)" : ""}`);
    } catch (e) {
      showError("MIDI ファイルを読み込めませんでした", e);
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
    let start = bar.tick;
    // 小節頭が前のクリップに食われていたらその終端から
    const covering = track.clips.find((c) => start >= c.start && start < c.start + c.length);
    if (covering) start = covering.start + covering.length;
    // 長さ: 4 小節ぶん(拍子を考慮)。次のクリップの手前まででクランプ
    let length = 0;
    for (let i = bar.index; i < Math.min(bar.index + 4, barList.length); i++) {
      length += barList[i].len;
    }
    if (length === 0) length = 3840 * 4;
    const nextStart = track.clips
      .map((c) => c.start)
      .filter((s) => s >= start + 1)
      .sort((a, b) => a - b)[0];
    if (nextStart !== undefined) length = Math.min(length, nextStart - start);
    if (length < 240) return; // 置く隙間がない

    const clipId = newClipId();
    const name = `クリップ ${bar.index + 1}`;
    api
      .applyEdit(
        [
          {
            op: "add_clip",
            track: track.id,
            clip: { id: clipId, name, start, length, kind: "midi", notes: [] },
          },
        ],
        `${track.name} の ${bar.index + 1} 小節目にクリップを追加`,
      )
      .then(() => {
        pianoRollStore.focus = {
          clipId,
          clipName: name,
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
      api.applyEdit(cmds, `クリップ ${cmds.length} 個を移動`).catch(() => {});
      return;
    }
    suppressOpen = true;
    setTimeout(() => (suppressOpen = false), 400);
    if (d.mode === "resize") {
      if (d.previewLength !== d.clip.length) {
        api
          .applyEdit(
            [{ op: "resize_clip", id: d.clip.id, length: d.previewLength }],
            `${d.clip.name} の長さを ${(d.previewLength / (project.ppq * 4)).toFixed(2)} 小節相当に変更`,
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
            ? `${d.clip.name} を ${destName} へ移動`
            : `${d.clip.name} を移動`,
        )
        .catch(() => {});
    }
  }

  // ---- クリップの選択・分割・削除・コピー ----

  let selectedClips = $state<Set<string>>(new Set());

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
        ids.length === 1 ? "クリップを削除" : `クリップ ${ids.length} 個を削除`,
      )
      .catch(() => {});
    selectedClips = new Set();
  }

  /// ループクリップの繰り返しを実際のノートに展開した replace_clip(ループは解除)。
  function expandLoopCommand(clip: Clip): Record<string, unknown> | null {
    if (clip.kind !== "midi" || !clip.loop || !clip.loop_len) return null;
    const L = clip.loop_len;
    const notes: Record<string, unknown>[] = [];
    for (let offset = 0; offset < clip.length; offset += L) {
      for (const n of clip.notes) {
        if (n.pos >= L) continue;
        const pos = n.pos + offset;
        if (pos >= clip.length) continue;
        const end = Math.min(n.pos + n.dur, L) + offset;
        notes.push({ ...n, id: newNoteId(), pos, dur: Math.min(end, clip.length) - pos });
      }
    }
    const c = JSON.parse(JSON.stringify(clip)) as Record<string, unknown>;
    c.notes = notes;
    c.loop = false;
    delete c.loop_len;
    return { op: "replace_clip", id: clip.id, clip: c };
  }

  function setLoop(clip: Clip, on: boolean) {
    if (clip.kind !== "midi") return;
    api
      .applyEdit(
        [{ op: "set_clip_loop", id: clip.id, loop_len: on ? clip.length : null }],
        on ? `${clip.name} をループにする` : `${clip.name} のループを解除`,
      )
      .catch(() => {});
  }

  /// `tick` の位置のテンポ(BPM)
  function bpmAt(tick: number): number {
    let bpm = project.tempo_map[0]?.bpm ?? 120;
    for (const e of project.tempo_map) {
      if (e.tick <= tick) bpm = e.bpm;
    }
    return bpm;
  }

  // ---- 音声クリップのフェードと音量(下の角のつまみで長さ、下の中央のつまみで音量) ----

  /// ドラッグ中のフェード・音量(離すまで表示と試聴だけ)
  let clipHandle = $state<{
    clip: Clip;
    which: "in" | "out" | "gain";
    startX: number;
    startY: number;
    orig: number;
    value: number;
  } | null>(null);

  /// `tick` の位置の 1 tick のミリ秒
  function msPerTick(tick: number): number {
    return 60000 / (bpmAt(tick) * project.ppq);
  }

  function clipMs(clip: Clip): number {
    return clip.length * msPerTick(clip.start);
  }

  function fadeMs(clip: Clip, which: "in" | "out"): number {
    if (clip.kind !== "audio") return 0;
    if (clipHandle?.clip.id === clip.id && clipHandle.which === which) return clipHandle.value;
    return (which === "in" ? clip.fade_in_ms : clip.fade_out_ms) ?? 0;
  }

  function clipGain(clip: Clip): number {
    if (clip.kind !== "audio") return 0;
    if (clipHandle?.clip.id === clip.id && clipHandle.which === "gain") return clipHandle.value;
    return clip.gain_db ?? 0;
  }

  function fadePx(clip: Clip, which: "in" | "out"): number {
    return (fadeMs(clip, which) / msPerTick(which === "in" ? clip.start : clip.start + clip.length)) * pxPerTick;
  }

  function handleCommand(h: NonNullable<typeof clipHandle>): unknown {
    const key = h.which === "in" ? "fade_in_ms" : h.which === "out" ? "fade_out_ms" : "gain_db";
    return { op: "replace_clip", id: h.clip.id, clip: { ...h.clip, [key]: h.value } };
  }

  function onHandleDown(e: PointerEvent, clip: Clip, which: "in" | "out" | "gain") {
    if (e.button !== 0 || clip.kind !== "audio") return;
    e.stopPropagation();
    const orig = which === "gain" ? (clip.gain_db ?? 0) : ((which === "in" ? clip.fade_in_ms : clip.fade_out_ms) ?? 0);
    clipHandle = { clip, which, startX: e.clientX, startY: e.clientY, orig, value: orig };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }

  function onHandleMove(e: PointerEvent) {
    const h = clipHandle;
    if (!h) return;
    let v: number;
    if (h.which === "gain") {
      // 上へ 1px = +0.1dB(Shift で細かく)。-36〜+12 dB
      const per = e.shiftKey ? 0.02 : 0.1;
      v = Math.round((h.orig - (e.clientY - h.startY) * per) * 10) / 10;
      v = Math.min(12, Math.max(-36, v));
    } else {
      const other = fadeMs(h.clip, h.which === "in" ? "out" : "in");
      const dx = (e.clientX - h.startX) * (h.which === "in" ? 1 : -1);
      const tickAt = h.which === "in" ? h.clip.start : h.clip.start + h.clip.length;
      v = h.orig + (dx / pxPerTick) * msPerTick(tickAt);
      // フェードイン + アウトはクリップの長さまで
      v = Math.round(Math.min(Math.max(0, clipMs(h.clip) - other), Math.max(0, v)));
    }
    if (v === h.value) return;
    h.value = v;
    api.previewEdit([handleCommand(h)]);
  }

  function onHandleUp() {
    const h = clipHandle;
    if (!h) return;
    clipHandle = null;
    if (h.value === h.orig) return;
    const label =
      h.which === "gain"
        ? `${h.clip.name} の音量を ${h.value > 0 ? "+" : ""}${h.value.toFixed(1)} dB に`
        : `${h.clip.name} のフェード${h.which === "in" ? "イン" : "アウト"}を ${Math.round(h.value)}ms に`;
    api.applyEdit([handleCommand(h)], label).catch(() => {});
  }

  /** つまみのダブルクリックで元に戻す(フェードなし / 0 dB) */
  function resetHandle(clip: Clip, which: "in" | "out" | "gain") {
    if (clip.kind !== "audio") return;
    const key = which === "in" ? "fade_in_ms" : which === "out" ? "fade_out_ms" : "gain_db";
    if ((clip[key] ?? 0) === 0) return;
    const what = which === "gain" ? "音量を 0 dB に戻す" : `フェード${which === "in" ? "イン" : "アウト"}をなくす`;
    api.applyEdit([{ op: "replace_clip", id: clip.id, clip: { ...clip, [key]: 0 } }], `${clip.name} の${what}`).catch(() => {});
  }

  function followBpm(clip: Clip): number | null {
    return clip.kind === "audio" && clip.stretch?.mode === "follow" ? clip.stretch.original_bpm : null;
  }

  /// 音声クリップのテンポ追従。ON のときは今のテンポ(クリップ先頭)で録った素材として扱う
  function setFollow(targets: Clip[], on: boolean) {
    const cmds = targets
      .filter((c) => c.kind === "audio")
      .map((c) => ({
        op: "set_clip_stretch",
        id: c.id,
        stretch: on ? { mode: "follow", original_bpm: bpmAt(c.start) } : { mode: "none" },
      }));
    if (cmds.length === 0) return;
    const label = on
      ? `${cmds.length === 1 ? targets[0].name : `音声クリップ ${cmds.length} 個`} をテンポに追従させる`
      : "テンポ追従を解除";
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
      showError("テンポを検出できませんでした", e);
      return;
    } finally {
      detectingTempo = null;
    }
    if (failed.length > 0) {
      showToast("warn", `テンポを検出できませんでした(拍のはっきりしない音か、短すぎます): ${failed.join("、")}`);
    }
    if (cmds.length === 0) return;
    const label =
      cmds.length === 1
        ? `${found[0]} の素材としてテンポに追従させる`
        : `音声クリップ ${cmds.length} 個をテンポに追従させる(元のテンポを検出)`;
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
        `「${r.track_name}」を作りました(音源: ${r.instrument}${r.reverb ? " + リバーブ" : ""}、近さ: ${r.verdict}、距離 ${r.initial_distance.toFixed(2)} → ${r.distance.toFixed(2)})。\n` +
          "音作りビューでつまみを微調整できます(Ctrl+Z で取り消し)。",
      );
    } catch (e) {
      showError("似た音を作れませんでした", e);
    } finally {
      matching = null;
    }
  }

  function expandLoop(clip: Clip) {
    const cmd = expandLoopCommand(clip);
    if (!cmd) return;
    api.applyEdit([cmd], `${clip.name} の繰り返しをノートに展開`).catch(() => {});
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
      .applyEdit(cmds, n === 1 ? "クリップを分割" : `クリップ ${n} 個を分割`)
      .catch(() => {});
    return true;
  }

  /// クリップを新しい ID(ノートも新 ID)で複製した add_clip 用の JSON を作る
  function cloneClip(clip: Clip, start: number): Record<string, unknown> {
    const c = JSON.parse(JSON.stringify(clip)) as Record<string, unknown>;
    c.id = newClipId();
    c.start = Math.max(0, Math.round(start));
    if (clip.kind === "midi") {
      c.notes = clip.notes.map((n) => ({ ...n, id: newNoteId() }));
    }
    return c;
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
    api.applyEdit(cmds, `クリップ ${cmds.length} 個を貼り付け`).catch(() => {});
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
    api.applyEdit(cmds, `クリップ ${cmds.length} 個を複製`).catch(() => {});
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

  function menuAction(
    action:
      | "split"
      | "split-head"
      | "dup"
      | "copy"
      | "cut"
      | "delete"
      | "loop-on"
      | "loop-off"
      | "expand"
      | "follow-on"
      | "follow-detect"
      | "follow-off"
      | "sep-builtin"
      | "sep-demucs"
      | "match"
      | "similar",
  ) {
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
    return `${b.index + 1} 小節 ${beat} 拍`;
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
    name: "マスター",
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
    const name = project.tracks[from]?.name ?? "トラック";
    api
      .applyEdit([{ op: "move_track", id, to_index: to }], `${name} を ${to + 1} 番目へ移動`)
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
    const name = value.trim();
    if (!name || name === track.name) return;
    api
      .applyEdit(
        [{ op: "set_track_prop", id: track.id, prop: "name", value: name }],
        `トラック名を「${track.name}」から「${name}」に変更`,
      )
      .catch(() => {});
  }

  /** 入力欄を開いたら全選択してフォーカス */
  function focusSelect(el: HTMLInputElement) {
    el.focus();
    el.select();
  }

  const TRACK_COLORS = ["#25bdb1", "#5da2e8", "#b07ce8", "#e87ca8", "#e8a07c", "#e8d27c", "#7cc47c", null];

  function setTrackColor(color: string | null) {
    const menu = trackMenu;
    trackMenu = null;
    if (!menu) return;
    const track = project.tracks[menu.index];
    api
      .applyEdit(
        [{ op: "set_track_prop", id: menu.trackId, prop: "color", value: color }],
        `${track?.name ?? "トラック"} の色を${color ? "変更" : "元に戻す"}`,
      )
      .catch(() => {});
  }

  /// トラックを複製して直後に置く(クリップ・ノート・エフェクトの ID は新しく振る)
  function duplicateTrack() {
    const menu = trackMenu;
    trackMenu = null;
    if (!menu) return;
    const src = project.tracks[menu.index];
    if (!src) return;
    const copy: Track = JSON.parse(JSON.stringify(src));
    copy.id = newTrackId();
    copy.name = `${src.name} のコピー`;
    copy.solo = false;
    const fxIds = new Map<string, string>();
    for (const fx of copy.effects) {
      const id = newFxId();
      fxIds.set(fx.id, id);
      fx.id = id;
    }
    // エフェクトのつながり(ノード表示の線)も新しい ID に
    if (copy.fx_links) {
      copy.fx_links = copy.fx_links.map((l) => ({ ...l, from: fxIds.get(l.from) ?? l.from, to: fxIds.get(l.to) ?? l.to }));
    }
    for (const lane of copy.automation) {
      const m = lane.target.match(/^fx\/([^/]+)\/(.*)$/);
      if (m && fxIds.has(m[1])) lane.target = `fx/${fxIds.get(m[1])}/${m[2]}`;
    }
    for (const c of copy.clips) {
      c.id = newClipId();
      if (c.kind === "midi") for (const n of c.notes) n.id = newNoteId();
    }
    api
      .applyEdit([{ op: "add_track", track: copy, index: menu.index + 1 }], `${src.name} を複製`)
      .catch(() => {});
  }

  /// トラックを音声にする(描き出しに曲の長さの数分の 1 かかる)
  let bouncing = $state<string | null>(null);
  async function bounceTrack() {
    const menu = trackMenu;
    trackMenu = null;
    if (!menu || bouncing) return;
    const track = project.tracks[menu.index];
    bouncing = menu.trackId;
    showToast("ok", `「${track?.name ?? "トラック"}」を音声に描き出しています…`);
    try {
      const r = await api.bounceTrack(menu.trackId);
      showToast("ok", `「${track?.name}」を音声にしました(${r.seconds.toFixed(1)} 秒。元のトラックはミュート、Ctrl+Z で戻せます)`);
    } catch (e) {
      showError("音声にできませんでした", e);
    } finally {
      bouncing = null;
    }
  }

  function deleteTrack() {
    const menu = trackMenu;
    trackMenu = null;
    if (!menu) return;
    const track = project.tracks[menu.index];
    api
      .applyEdit(
        [{ op: "remove_track", id: menu.trackId }],
        `${track?.name ?? "トラック"} を削除`,
      )
      .catch(() => {});
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
    api.clapOpenGui(trackId).catch((err) => showError("プラグインの画面を開けませんでした", err));
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
    else addTrack(kind);
  }

  /// マスター音量のドラッグ中の値(離すまで表示と試聴だけ)
  let masterDrag = $state<number | null>(null);

  function setMasterVolume(e: Event) {
    const v = Number((e.currentTarget as HTMLInputElement).value);
    masterDrag = null;
    api
      .applyEdit([{ op: "set_master_volume", volume_db: v }], `マスター音量を ${v.toFixed(1)} dB に変更`)
      .catch(() => {});
  }

  const KIND_ICON = { midi: "piano", audio: "audio-lines", bus: "merge" } as const;
  const KIND_LABEL = { midi: "MIDI トラック", audio: "音声トラック", bus: "バス" } as const;

  function addTrack(kind: "midi" | "audio" | "bus" = "midi") {
    const id = newTrackId();
    if (kind === "bus") {
      // バス: 共有リバーブとして使えるよう、リバーブ(ウェット 100%)を挿して作る
      const n = project.tracks.filter((t) => t.kind === "bus").length + 1;
      api
        .applyEdit(
          [
            { op: "add_track", track: { id, name: `リバーブ バス ${n}`, kind } },
            {
              op: "add_effect",
              track: id,
              effect: { id: newFxId(), type: "builtin", name: "reverb", params: { mix: 1.0 } },
            },
          ],
          "バスを追加",
        )
        .catch(() => {});
      return;
    }
    const name =
      kind === "audio"
        ? `音声 ${project.tracks.filter((t) => t.kind === "audio").length + 1}`
        : `トラック ${project.tracks.length + 1}`;
    api
      .applyEdit(
        [{ op: "add_track", track: { id, name, kind } }],
        kind === "audio" ? "音声トラックを追加" : "トラックを追加",
      )
      .catch(() => {});
  }

  /// このバスへ送っているトラックの数
  function sendersOf(bus: Track): number {
    return project.tracks.filter((t) => t.sends?.some((s) => s.target === bus.id)).length;
  }

  function toggleSolo(t: Track) {
    api
      .applyEdit(
        [{ op: "set_track_prop", id: t.id, prop: "solo", value: !t.solo }],
        `${t.name} のソロを${t.solo ? "解除" : "オン"}`,
      )
      .catch(() => {});
  }
</script>

<div class="timeline" bind:this={root} style="--head-w:{HEAD_W}px;--track-h:{timelineLayout.trackH}px">
  <!-- 再生ヘッド -->
  <div class="playhead" style="left:{playheadPx}px"></div>

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
  {#if project.sections && project.sections.length > 0}
    <div class="section-row">
      <div class="track-head section-head"></div>
      <div class="lane" style="width:{totalPx}px">
        {#each project.sections as sec, i (sec.tick)}
          {@const next = project.sections?.[i + 1]?.tick}
          {@const at = markerDrag?.tick === sec.tick ? markerDrag.to : sec.tick}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="section-band"
            class:dragging={markerDrag?.tick === sec.tick && markerDrag.moved}
            style="left:{at * pxPerTick}px;{next !== undefined && at === sec.tick
              ? `width:${(next - sec.tick) * pxPerTick}px`
              : at === sec.tick
                ? `right:0`
                : `width:120px`}"
            title={`${sec.name}(${barAtTick(barList, sec.tick).index + 1} 小節目〜)\nクリックでこの区間を選択 / ドラッグで移動 / ダブルクリックで名前を変更 / 右クリックで削除`}
            onpointerdown={(e) => onMarkerDown(e, sec)}
            onpointermove={onMarkerMove}
            onpointerup={(e) => onMarkerUp(e, sec, next)}
            ondblclick={() => (renamingMarker = sec.tick)}
            oncontextmenu={(e) => {
              e.preventDefault();
              removeMarker(sec.tick);
            }}
          >
            {#if renamingMarker === sec.tick}
              <input
                class="marker-input"
                value={sec.name}
                use:focusSelect
                onpointerdown={(e) => e.stopPropagation()}
                onkeydown={(e) => {
                  if (e.key === "Enter") renameMarker(sec.tick, e.currentTarget.value);
                  else if (e.key === "Escape") renamingMarker = null;
                }}
                onblur={(e) => renameMarker(sec.tick, e.currentTarget.value)}
              />
            {:else}
              {sec.name}
            {/if}
          </div>
        {/each}
      </div>
    </div>
  {/if}

  <!-- 小節ルーラー(クリックでシーク、ドラッグで範囲選択) -->
  <div class="ruler-row" style="top:{project.sections && project.sections.length > 0 ? SECTION_ROW_H : 0}px">
    <div class="track-head ruler-head">
      <div class="zoom" title="横の拡大・縮小(タイムラインの上で Ctrl+ホイールでも)">
        <button class="btn sm icon-only" onclick={() => zoomBy(1 / 1.5)} disabled={timelineZoom.value <= TIMELINE_ZOOM_MIN} aria-label="縮小" title="縮小"
          ><Icon name="zoom-out" size={13} /></button
        >
        <button class="btn sm icon-only" onclick={() => zoomBy(1.5)} disabled={timelineZoom.value >= TIMELINE_ZOOM_MAX} aria-label="拡大" title="拡大"
          ><Icon name="zoom-in" size={13} /></button
        >
        <button class="btn sm" onclick={zoomToFit} title="曲全体が横に収まるようにする">全体</button>
        <button
          class="btn sm icon-only"
          class:on={layoutMenu}
          onclick={() => (layoutMenu = !layoutMenu)}
          aria-label="トラックの表示の大きさ"
          title="トラックの高さと見出しの幅"><Icon name="rows-2" size={13} /></button
        >
        {#if Math.abs(timelineZoom.value - 1) > 0.01}
          <button class="btn sm" onclick={() => setZoom(1)} title="元の拡大率(100%)に戻す">{Math.round(timelineZoom.value * 100)}%</button>
        {/if}
      </div>
      {#if layoutMenu}
        <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
        <div class="menu-backdrop" role="presentation" onclick={() => (layoutMenu = false)}></div>
        <div class="layout-pop">
          <label>
            <span>トラックの高さ <b>{timelineLayout.trackH}px</b></span>
            <input
              type="range"
              min={TIMELINE_TRACK_H.min}
              max={TIMELINE_TRACK_H.max}
              step="2"
              bind:value={timelineLayout.trackH}
              onchange={saveTimelineLayout}
              aria-label="トラックの高さ"
            />
          </label>
          <label>
            <span>見出しの幅 <b>{timelineLayout.headW}px</b></span>
            <input
              type="range"
              min={TIMELINE_HEAD_W.min}
              max={TIMELINE_HEAD_W.max}
              step="4"
              bind:value={timelineLayout.headW}
              onchange={saveTimelineLayout}
              aria-label="見出しの幅"
            />
          </label>
          <button
            class="btn sm"
            onclick={() => {
              timelineLayout.trackH = TIMELINE_TRACK_H.def;
              timelineLayout.headW = TIMELINE_HEAD_W.def;
              saveTimelineLayout();
            }}>元に戻す</button
          >
        </div>
      {/if}
    </div>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="lane seekable"
      style="width:{totalPx}px"
      onpointerdown={onRulerDown}
      onpointermove={onRulerMove}
      onpointerup={onRulerUp}
      oncontextmenu={onRulerContext}
      title="クリックで移動、ドラッグで範囲選択、右クリックでその小節から拍子を変更"
    >
      {#each barList as bar (bar.index)}
        <div class="bar-mark" class:quiet={bar.index % barLabelEvery !== 0} style="left:{bar.tick * pxPerTick}px">
          {#if bar.index % barLabelEvery === 0}{bar.index + 1}{/if}{#if barLabelEvery === 1 && chordAt.get(bar.tick)}<span class="chord-chip">{chordAt.get(bar.tick)}</span>{/if}{#if barLabelEvery === 1 && tempoAt.get(bar.tick)}<span
              class="tempo-chip"
              title="この小節からのテンポ(右クリックで変更・削除)">♩={tempoAt.get(bar.tick)}</span
            >{/if}{#if bar.sigChange && barLabelEvery === 1}<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions --><span
              class="sig-chip"
              title="クリックで拍子を編集・削除"
              onpointerdown={(e) => e.stopPropagation()}
              onpointerup={(e) => e.stopPropagation()}
              onclick={(e) => openSigMenu(e, bar.index)}>{bar.num}/{bar.den}</span
            >{/if}
        </div>
      {/each}
    </div>
  </div>

  <!-- マスター: 曲全体の音量・オートメーション・エフェクト。見つけやすいよう、トラックの一番上に置く
       (以前は一番下で、音量のつまみも短かった) -->
  <div class="track-row master-row">
    <div class="track-head">
      <div class="head-row">
        <span class="track-name master-name" title="曲全体の音量・エフェクト(書き出しにも入る)">マスター</span>
        <span class="db master-db">{(masterDrag ?? project.master.volume_db).toFixed(1)} dB</span>
        <button
          class="btn sm icon"
          class:on={autoLanes[MASTER_FOCUS_ID] !== undefined}
          onclick={() => toggleAutoLane(MASTER_FOCUS_ID)}
          title={`マスターのオートメーション(フェードアウト、マスターのエフェクトの時間変化)${masterLaneCount > 0 ? `。描いてあるレーン ${masterLaneCount} 本` : ""}`}
          aria-label="マスターのオートメーション"><Icon name="spline" /></button
        >
        <button
          class="btn sm icon"
          class:on={soundDesignStore.focus?.trackId === MASTER_FOCUS_ID}
          onclick={() =>
            (soundDesignStore.focus =
              soundDesignStore.focus?.trackId === MASTER_FOCUS_ID ? null : { trackId: MASTER_FOCUS_ID, trackName: "マスター" })}
          title={`マスターのエフェクト(曲全体に掛かるコンプ・EQ・リバーブなど)${project.master.effects.length > 0 ? `。${project.master.effects.length} 個` : ""}`}
          aria-label="マスターのエフェクト"><Icon name="sliders-horizontal" /></button
        >
      </div>
      <input
        class="vol master-vol"
        type="range"
        min="-40"
        max="6"
        step="0.5"
        value={project.master.volume_db}
        title="マスター音量(曲全体。書き出しにも入る。ダブルクリックで 0 dB)"
        aria-label="マスター音量"
        oninput={(e) => {
          masterDrag = Number(e.currentTarget.value);
          api.previewEdit([{ op: "set_master_volume", volume_db: masterDrag }]);
        }}
        onchange={setMasterVolume}
        ondblclick={() => {
          masterDrag = null;
          api.applyEdit([{ op: "set_master_volume", volume_db: 0 }], "マスター音量を 0.0 dB に変更").catch(() => {});
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
      トラックがありません。AI に「トラックを追加して」と頼んでみてください。
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
    <div class="track-row" class:alt={ti % 2 === 1}>
      <!-- 見出し: 1 段目 = つかむ所・種類・名前・⋯ / 2 段目 = M・S・音量 / 3 段目 = 音源と固定の 3 つ
           (アーム・オートメーション・インスペクター。無いものは空けて、どのトラックでも同じ位置に) -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="track-head"
        style={track.color ? `--tc:${track.color}` : ""}
        oncontextmenu={(e) => openTrackMenu(e, track)}
      >
        <div class="head-row">
          <span class="grip" role="button" tabindex="-1" aria-label="並べ替え" title="つかんで上下にドラッグで並べ替え" onpointerdown={(e) => onGripDown(e, track, ti)}
            ><Icon name="grip-vertical" size={14} /></span
          >
          <span class="kind-ic" title={KIND_LABEL[track.kind]}><Icon name={KIND_ICON[track.kind]} size={14} /></span>
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
            <div class="track-name" title={`${track.name}(ダブルクリックで名前を変更)`} ondblclick={() => (renaming = track.id)}>
              {track.name}
            </div>
          {/if}
          <button
            class="btn sm icon ghost"
            onclick={(e) => openTrackMenuAt(e, track)}
            title="トラックのメニュー(名前・色・並べ替え・複製・音声にする・削除)"
            aria-label="トラックのメニュー"><Icon name="ellipsis" /></button
          >
        </div>
        <div class="head-row">
          <button class="btn letter" class:m-on={track.mute} onclick={() => toggleMute(track)} title="ミュート" aria-pressed={track.mute}
            >M</button
          >
          <button class="btn letter" class:s-on={track.solo} onclick={() => toggleSolo(track)} title="ソロ" aria-pressed={track.solo}
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
            title={hasVolumeLane(track) ? "音量オートメーション使用中(フェーダーより優先されます)" : "音量"}
            aria-label="音量"
            onchange={(e) => setVolume(track, e)}
          />
          <span class="db" title="音量(dB)">{volumeText(track)}</span>
        </div>
        <div class="head-row">
          {#if track.kind === "bus"}
            <span class="dev plain" title="インスペクターの「送り」で、各トラックからこのバスへ送る量を決めます"
              >受けている: {sendersOf(track)} 本</span
            >
          {:else if track.kind === "audio"}
            <span class="dev plain">音声</span>
          {:else}
            <button
              class="dev"
              onclick={(e) => openPicker(e, track)}
              title={track.device ? "クリックで音源を変える" : "音源未設定(既定の subtractive で発音)。クリックで選ぶ"}
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
              title="MIDI キーボードでこのトラックを弾く(ON の間は録音が MIDI 録音になります)"
              aria-label="MIDI キーボードで弾く"
              aria-pressed={midiArmStore.trackId === track.id}><Icon name="keyboard-music" /></button
            >
          {:else}
            <span class="slot"></span>
          {/if}
          <button
            class="btn sm icon"
            class:on={autoLanes[track.id] !== undefined}
            onclick={() => toggleAutoLane(track.id)}
            title="オートメーション(音量・パン・つまみを時間で動かす)"
            aria-label="オートメーション"
            aria-pressed={autoLanes[track.id] !== undefined}><Icon name="spline" /></button
          >
          <button
            class="btn sm icon"
            class:on={soundDesignStore.focus?.trackId === track.id}
            onclick={() =>
              (soundDesignStore.focus =
                soundDesignStore.focus?.trackId === track.id ? null : { trackId: track.id, trackName: track.name })}
            title="インスペクター(音源・エフェクト・送り)"
            aria-label="インスペクター"
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
          ? "バス: 他のトラックのセンドを受けて、エフェクト → 音量/パン → マスターへ(クリップは置けません)"
          : track.kind === "audio"
          ? "ダブルクリックで音声ファイル(WAV / MP3 等)をその小節に配置(録音はヘッダーの録音ボタン)"
          : track.clips.length === 0
            ? "ダブルクリックでクリップを作成してピアノロールを開く"
            : ""}
      >
        {#each barList as bar (bar.index)}
          <div class="grid-line" style="left:{bar.tick * pxPerTick}px"></div>
        {/each}
        {#each track.clips as clip (clip.id)}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="clip {clip.kind}"
            class:dragging={clipDrag?.moved &&
              (clipDrag.clip.id === clip.id || (clipDrag.mode === "move" && clipDrag.group.has(clip.id)))}
            class:selected={selectedClips.has(clip.id)}
            class:ai-changed={aiHighlight.clips.has(clip.id)}
            style={clipStyle(clip)}
            title={`${clip.name} (${clip.id})${clip.kind === "midi" ? " — ダブルクリックでピアノロール" : ""} / クリックで選択(Ctrl・Shift で複数)/ ドラッグで移動(Alt でスナップ解除)/ 右端で長さ変更 / 右クリックで分割・複製・削除 / S: 再生ヘッドで分割、Delete: 削除、Ctrl+C/X/V/D`}
            ondblclick={(e) => openPianoRoll(track, clip, e)}
            oncontextmenu={(e) => onClipContext(e, clip)}
            onpointerdown={(e) => onClipDown(e, track, clip)}
            onpointermove={onClipDragMove}
            onpointerup={onClipUp}
            onpointercancel={() => (clipDrag = null)}
          >
            <span class="clip-name"
              >{#if clip.kind === "audio"}<Icon name="audio-lines" size={12} />{:else if clip.loop && clip.loop_len}<span
                  title="ループのクリップ"><Icon name="infinity" size={12} /></span
                >{/if}{clip.name}{#if followBpm(clip) !== null}<span class="follow" title="テンポに追従中(元の素材の BPM)"
                  ><Icon name="move-horizontal" size={12} />{followBpm(clip)}</span
                >{/if}</span
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
              {@const fiPx = Math.min(fadePx(clip, "in"), clip.length * pxPerTick)}
              {@const foPx = Math.min(fadePx(clip, "out"), clip.length * pxPerTick)}
              {@const w = Math.max(clip.length * pxPerTick, 8)}
              {@const gain = clipGain(clip)}
              {#if fiPx > 0 || foPx > 0}
                <svg class="fade-shade" width={w} height="100%" viewBox="0 0 {w} 100" preserveAspectRatio="none" aria-hidden="true">
                  {#if fiPx > 0}<polygon points="0,0 {fiPx},0 0,100" />{/if}
                  {#if foPx > 0}<polygon points="{w - foPx},0 {w},0 {w},100" />{/if}
                </svg>
              {/if}
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <span
                class="fade-h in"
                class:active={clipHandle?.clip.id === clip.id && clipHandle.which === "in"}
                style="left:{fiPx}px"
                title="フェードイン {Math.round(fadeMs(clip, 'in'))}ms(横にドラッグ。ダブルクリックでなくす)"
                onpointerdown={(e) => onHandleDown(e, clip, "in")}
                onpointermove={onHandleMove}
                onpointerup={onHandleUp}
                onpointercancel={() => (clipHandle = null)}
                ondblclick={(e) => {
                  e.stopPropagation();
                  resetHandle(clip, "in");
                }}
              ></span>
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <span
                class="fade-h out"
                class:active={clipHandle?.clip.id === clip.id && clipHandle.which === "out"}
                style="right:{foPx}px"
                title="フェードアウト {Math.round(fadeMs(clip, 'out'))}ms(横にドラッグ。ダブルクリックでなくす)"
                onpointerdown={(e) => onHandleDown(e, clip, "out")}
                onpointermove={onHandleMove}
                onpointerup={onHandleUp}
                onpointercancel={() => (clipHandle = null)}
                ondblclick={(e) => {
                  e.stopPropagation();
                  resetHandle(clip, "out");
                }}
              ></span>
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <span
                class="gain-h"
                class:active={clipHandle?.clip.id === clip.id && clipHandle.which === "gain"}
                class:set={gain !== 0}
                title="クリップの音量(上下にドラッグ、Shift で細かく。ダブルクリックで 0 dB)"
                onpointerdown={(e) => onHandleDown(e, clip, "gain")}
                onpointermove={onHandleMove}
                onpointerup={onHandleUp}
                onpointercancel={() => (clipHandle = null)}
                ondblclick={(e) => {
                  e.stopPropagation();
                  resetHandle(clip, "gain");
                }}>{gain > 0 ? "+" : ""}{gain.toFixed(1)} dB</span
              >
              <button
                class="transcribe"
                disabled={transcribing !== null}
                title="譜起こし(単旋律): 鼻歌・歌・単音の音声を MIDI クリップにする"
                onpointerdown={(e) => e.stopPropagation()}
                ondblclick={(e) => e.stopPropagation()}
                onclick={(e) => {
                  e.stopPropagation();
                  transcribe(track, clip);
                }}
              >
                {#if transcribing === clip.id}<Icon name="loader-circle" size={12} />{:else}<Icon name="music" size={12} />{/if}
              </button>
              <button
                class="transcribe poly"
                disabled={transcribing !== null}
                title="譜起こし(和音): ピアノ・ギターのコードや伴奏入りの音声を MIDI クリップにする(学習済みモデル basic-pitch)"
                onpointerdown={(e) => e.stopPropagation()}
                ondblclick={(e) => e.stopPropagation()}
                onclick={(e) => {
                  e.stopPropagation();
                  transcribe(track, clip, "poly");
                }}
              >
                <Icon name="list-music" size={12} />
              </button>
            {/if}
            {#if separating === clip.id}
              <div class="clip-busy">パートに分離中…</div>
            {:else if detectingTempo === clip.id}
              <div class="clip-busy">テンポを検出中…</div>
            {:else if matching === clip.id}
              <div class="clip-busy">音色を合わせています…</div>
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
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="menu-backdrop" role="presentation" onclick={() => (sigMenu = null)} oncontextmenu={(e) => { e.preventDefault(); sigMenu = null; }}></div>
    <div class="track-menu sig-menu" use:keepInView style="left:{sigMenu.x}px;top:{sigMenu.y}px">
      <div class="preset-title">{sigMenu.barIndex + 1} 小節目から拍子を変更</div>
      <div class="sig-form">
        <input
          class="sig-num"
          type="number"
          min="1"
          max="32"
          bind:value={sigMenu.num}
          onkeydown={(e) => e.key === "Enter" && applySig()}
        />
        <span>/</span>
        <select bind:value={sigMenu.den}>
          {#each DENS as d (d)}
            <option value={String(d)}>{d}</option>
          {/each}
        </select>
        <button class="sig-apply" onclick={applySig}>適用</button>
      </div>
      <div class="sig-presets">
        {#each ["4/4", "3/4", "6/8", "7/8", "5/4", "12/8"] as p (p)}
          <button
            class="sig-preset"
            onclick={() => {
              if (!sigMenu) return;
              const [n, d] = p.split("/");
              sigMenu.num = n;
              sigMenu.den = d;
              applySig();
            }}>{p}</button
          >
        {/each}
      </div>
      {#if sigMenu.barIndex > 0 && project.time_sig_map.some((e) => e.tick === barList[sigMenu!.barIndex].tick)}
        <div class="menu-sep"></div>
        <button class="danger" onclick={removeSig}><Icon name="trash-2" />この拍子の変更を削除(前の拍子に戻す)</button>
      {/if}
      <div class="menu-note">ノートの位置は変わらず、この小節から先の小節線だけが変わります(Ctrl+Z で戻せます)</div>
      <div class="menu-sep"></div>
      <div class="preset-title">{sigMenu.barIndex === 0 ? "曲の頭のテンポ" : `${sigMenu.barIndex + 1} 小節目からテンポを変更`}</div>
      <div class="sig-form">
        <input
          class="sig-num tempo-num"
          type="number"
          min="20"
          max="300"
          step="0.5"
          bind:value={sigMenu.bpm}
          onkeydown={(e) => e.key === "Enter" && applyTempo()}
          aria-label="テンポ(BPM)"
        />
        <span>BPM</span>
        <button class="sig-apply" onclick={applyTempo}>適用</button>
      </div>
      {#if sigMenu.barIndex > 0 && project.tempo_map.some((e) => e.tick === barList[sigMenu!.barIndex].tick)}
        <button class="danger" onclick={removeTempo}><Icon name="trash-2" />このテンポの変更を削除(前のテンポに戻す)</button>
      {/if}
      <div class="menu-note">ノートは拍の位置のまま、この小節から先の速さが変わります</div>
      <div class="menu-sep"></div>
      <div class="preset-title">
        {markerHere ? `マーカー「${markerHere.name}」` : `${sigMenu.barIndex + 1} 小節目にマーカーを置く`}
      </div>
      <div class="sig-form">
        <input
          class="marker-name"
          placeholder={markerHere ? "新しい名前" : "名前(例: サビ)"}
          bind:value={markerName}
          onkeydown={(e) => e.key === "Enter" && putMarker(sigMenu!.barIndex, markerName)}
        />
        <button class="sig-apply" onclick={() => putMarker(sigMenu!.barIndex, markerName)}>
          {markerHere ? "名前を変更" : "追加"}
        </button>
      </div>
      <div class="sig-presets">
        {#each MARKER_PRESETS as name (name)}
          <button class="sig-preset" onclick={() => putMarker(sigMenu!.barIndex, name)}>{name}</button>
        {/each}
      </div>
      {#if markerHere}
        <button class="danger" onclick={() => removeMarker(markerHere!.tick)}><Icon name="trash-2" />このマーカーを削除</button>
      {/if}
    </div>
  {/if}

  {#if similarFor}
    <SimilarPresetDialog {project} clip={similarFor} onClose={() => (similarFor = null)} />
  {/if}

  {#if clipMenu}
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="menu-backdrop" role="presentation" onclick={() => (clipMenu = null)} oncontextmenu={(e) => { e.preventDefault(); clipMenu = null; }}></div>
    <div class="track-menu clip-menu" use:keepInView style="left:{clipMenu.x}px;top:{clipMenu.y}px">
      {#if selectedClips.size > 1}
        <div class="menu-note">選択中のクリップ {selectedClips.size} 個が対象</div>
      {/if}
      {#if clipMenu.clip.kind === "midi"}
        {#if clipMenu.clip.loop && clipMenu.clip.loop_len}
          <button onclick={() => menuAction("loop-off")}><Icon name="infinity" />ループを解除</button>
          <button onclick={() => menuAction("expand")} title="繰り返しをノートに書き出して、1 回ずつ個別に編集できるようにする"
            ><Icon name="infinity" />繰り返しをノートに展開</button
          >
        {:else}
          <button onclick={() => menuAction("loop-on")} title="今の長さを繰り返す。右端を伸ばすと、その分だけ繰り返し鳴る"
            ><Icon name="infinity" />ループにする</button
          >
        {/if}
        <div class="menu-sep"></div>
      {:else}
        {#if followBpm(clipMenu.clip) !== null}
          <button onclick={() => menuAction("follow-off")} title={`今は ${followBpm(clipMenu.clip)} BPM の素材として伸縮中`}
            ><Icon name="move-horizontal" />テンポ追従を解除</button
          >
        {:else}
          <button
            class="rich"
            onclick={() => menuAction("follow-on")}
            title="テンポを変えても拍がずれないよう、音程を保ったまま伸縮する"
            ><Icon name="move-horizontal" /><span
              >テンポに追従させる<small>{bpmAt(clipMenu.clip.start)} BPM で録った素材として</small></span
            ></button
          >
          <button class="rich" onclick={() => menuAction("follow-detect")} disabled={detectingTempo !== null}
            ><Icon name="move-horizontal" /><span>テンポに追従させる<small>素材の元のテンポを自動で検出(取り込んだ曲・ループ素材)</small></span
            ></button
          >
        {/if}
        <button class="rich" onclick={() => menuAction("sep-builtin")} disabled={separating !== null}
          ><Icon name="layers" /><span>パートに分ける: 打楽器 / 音程楽器<small>内蔵。すぐ終わる</small></span></button
        >
        <button class="rich" onclick={() => menuAction("sep-demucs")} disabled={separating !== null}
          ><Icon name="layers" /><span>パートに分ける: ボーカル / ドラム / ベース / その他<small>Demucs(要インストール)。数分かかる</small></span
          ></button
        >
        <button class="rich" onclick={() => menuAction("match")} disabled={matching !== null}
          ><Icon name="wand-sparkles" /><span
            >この音に似せたシンセのトラックを作る<small>subtractive / fm / wavetable とリバーブを自動で探す。約 30 秒。単音のサンプル向け</small></span
          ></button
        >
        <button class="rich" onclick={() => menuAction("similar")}
          ><Icon name="search" /><span>この音に近い CLAP のプリセットを探す…<small>Surge XT など。読み込んでつまみも自動で詰められる</small></span
          ></button
        >
        <div class="menu-sep"></div>
      {/if}
      <button onclick={() => menuAction("split")}><Icon name="scissors" />ここで分割({barLabel(clipMenu.at)})</button>
      <button onclick={() => menuAction("split-head")}><Icon name="scissors" />再生ヘッドで分割<span class="key">S</span></button>
      <div class="menu-sep"></div>
      <button onclick={() => menuAction("dup")}><Icon name="copy" />複製(直後に並べる)<span class="key">Ctrl+D</span></button>
      <button onclick={() => menuAction("copy")}><span class="ic-space"></span>コピー<span class="key">Ctrl+C</span></button>
      <button onclick={() => menuAction("cut")}><span class="ic-space"></span>切り取り<span class="key">Ctrl+X</span></button>
      <div class="menu-sep"></div>
      <button class="danger" onclick={() => menuAction("delete")}><Icon name="trash-2" />削除<span class="key">Delete</span></button>
      <div class="menu-note">貼り付け(Ctrl+V)は再生ヘッドの位置・元のトラックに置かれます</div>
    </div>
  {/if}

  {#if trackMenu}
    {@const menuTrack = project.tracks[trackMenu.index]}
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="menu-backdrop" role="presentation" onclick={() => (trackMenu = null)} oncontextmenu={(e) => { e.preventDefault(); trackMenu = null; }}></div>
    <div class="track-menu" use:keepInView style="left:{trackMenu.x}px;top:{trackMenu.y}px">
      <button onclick={() => startRename(trackMenu!.trackId)}><Icon name="pencil" />名前を変更</button>
      <div class="color-row" role="group" aria-label="トラックの色">
        <Icon name="palette" />
        {#each TRACK_COLORS as c (c)}
          <button
            class="color-chip"
            class:none={!c}
            style={c ? `background:${c}` : ""}
            title={c ? `色: ${c}` : "色を元に戻す"}
            aria-label={c ? `色 ${c}` : "色を元に戻す"}
            onclick={() => setTrackColor(c)}
          ></button>
        {/each}
      </div>
      <div class="menu-sep"></div>
      <button disabled={trackMenu.index === 0} onclick={() => moveTrack(trackMenu!.index - 1)}><Icon name="arrow-up" />上へ移動</button>
      <button disabled={trackMenu.index >= project.tracks.length - 1} onclick={() => moveTrack(trackMenu!.index + 1)}
        ><Icon name="arrow-down" />下へ移動</button
      >
      <div class="menu-note">見出しの左端をつかんでドラッグしても並べ替えられます</div>
      <button onclick={duplicateTrack}><Icon name="copy" />複製</button>
      <button
        onclick={bounceTrack}
        disabled={!!bouncing || menuTrack?.kind === "bus"}
        title="エフェクト・音量・パン・送りの響きまで込みで音声に描き出し、直後に音声トラックとして置く(元はミュート)。CLAP の音源の曲をゲームで鳴らすとき・重いトラックを軽くするときに"
        ><Icon name="snowflake" />音声にする(フリーズ)</button
      >
      {#if menuTrack?.device?.type === "clap"}
        <button onclick={() => openClapGui(trackMenu!.trackId)}><Icon name="app-window" />プラグインの画面を開く</button>
      {/if}
      <div class="menu-sep"></div>
      <button class="danger" onclick={deleteTrack}><Icon name="trash-2" />削除<span class="key">Ctrl+Z で戻せます</span></button>
    </div>
  {/if}

  {#if addMenu}
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="menu-backdrop" role="presentation" onclick={() => (addMenu = null)} oncontextmenu={(e) => { e.preventDefault(); addMenu = null; }}></div>
    <div class="track-menu" use:keepInView style="left:{addMenu.x}px;top:{addMenu.y}px">
      <button class="rich" onclick={() => addFromMenu("midi")}
        ><Icon name="piano" /><span>MIDI トラック<small>ノートを打ち込む・AI に作らせる</small></span></button
      >
      <button class="rich" onclick={() => addFromMenu("audio")}
        ><Icon name="audio-lines" /><span>音声トラック<small>録音・音声ファイルを置く(空きをダブルクリック)</small></span></button
      >
      <button class="rich" onclick={() => addFromMenu("bus")}
        ><Icon name="merge" /><span>バス(リバーブ入り)<small>複数のトラックから送って響きを共有する</small></span></button
      >
      <div class="menu-sep"></div>
      <button class="rich" onclick={() => addFromMenu("midi-file")}
        ><Icon name="file-music" /><span>MIDI ファイルから…<small>パートごとにトラックを足す。空の曲ならテンポと拍子も</small></span></button
      >
    </div>
  {/if}


  <div class="add-track-row">
    <button class="btn add-track" onclick={openAddMenu} title="トラックを追加(MIDI・音声・バス・MIDI ファイル)"
      ><Icon name="plus" />トラックを追加<Icon name="chevron-down" size={14} /></button
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

  .sig-num.tempo-num {
    width: 64px;
  }

  /* 縮小して番号を間引いた小節は、区切りの線も薄く */
  .bar-mark.quiet {
    border-left-color: color-mix(in srgb, var(--border) 45%, transparent);
  }

  .zoom {
    display: flex;
    align-items: center;
    gap: 3px;
    height: 100%;
    padding: 0 6px;
  }

  .zoom .btn {
    height: 18px;
    padding: 0 5px;
    font-size: var(--fs-xs);
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

  .color-row {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 4px 10px;
    color: var(--text-dim);
    --icon-size: 15px;
  }

  .color-row :global(.icon) {
    margin-right: 5px;
  }

  .color-chip {
    width: 16px;
    height: 16px;
    padding: 0;
    border-radius: 50%;
    border: 1px solid var(--border);
  }

  .color-chip.none {
    background: repeating-linear-gradient(45deg, var(--bg), var(--bg) 3px, var(--border) 3px, var(--border) 5px);
  }

  .track-head {
    width: var(--head-w, 200px);
    flex-shrink: 0;
    padding: 5px 6px 5px 10px;
    background: var(--bg-panel);
    border-right: 1px solid var(--border);
    position: sticky;
    left: 0;
    z-index: 2;
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

  /* セクション行とルーラーは、縦にスクロールしても上端に残す(トラックが多いとシーク・範囲選択・
     小節番号の確認ができなくなっていた)。ルーラーの top はセクション行の高さ(マークアップで指定) */
  .section-row {
    display: flex;
    border-bottom: 1px solid var(--border);
    position: sticky;
    top: 0;
    z-index: 4;
  }

  .ruler-row {
    position: sticky;
    z-index: 4;
  }

  .section-row .track-head,
  .ruler-row .track-head {
    z-index: 5;
  }

  .section-head {
    padding: 0;
    height: 18px;
  }

  .section-row .lane {
    height: 18px;
    position: relative;
    background: var(--bg-panel);
  }

  .section-band {
    position: absolute;
    top: 2px;
    bottom: 2px;
    padding: 0 6px;
    font-size: 10px;
    line-height: 14px;
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    border-left: 2px solid var(--accent);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    cursor: grab;
    user-select: none;
  }

  .section-band.dragging {
    cursor: grabbing;
    opacity: 0.8;
    z-index: 1;
  }

  .marker-input {
    width: 100%;
    font: inherit;
    font-size: 10px;
    padding: 0 2px;
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--accent-dim);
  }

  .marker-name {
    flex: 1;
    min-width: 0;
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

  .layout-pop {
    position: absolute;
    top: 26px;
    left: 6px;
    z-index: 30;
    width: 220px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px 12px;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: var(--r-lg);
    box-shadow: var(--shadow-pop);
    font-size: var(--fs-sm);
  }

  .layout-pop label {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .layout-pop input {
    width: 100%;
    accent-color: var(--accent);
  }

  .layout-pop .btn {
    align-self: flex-end;
  }

  .track-name.master-name {
    flex: 0 0 auto;
    margin-right: 4px;
  }


  .track-row.alt .lane {
    background: var(--bg-lane-alt);
  }

  .grid-line {
    position: absolute;
    top: 0;
    bottom: 0;
    border-left: 1px solid rgba(255, 255, 255, 0.05);
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




  .menu-note {
    font-size: var(--fs-xs);
    color: var(--text-faint);
    padding: 2px 10px 4px 35px;
    max-width: 300px;
    line-height: 1.5;
  }

  .preset-title {
    font-size: 10px;
    color: var(--text-dim);
    padding: 2px 10px;
    letter-spacing: 0.05em;
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

  .clip-menu .danger,
  .sig-menu .danger {
    color: var(--warn);
  }

  /* 小節番号(.bar-mark)はクリックを素通しするが、拍子チップは押せるようにする */
  .sig-chip {
    cursor: pointer;
    pointer-events: auto;
  }

  .sig-form {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 2px 6px;
  }

  .sig-num {
    width: 52px;
  }

  .sig-apply {
    margin-left: auto;
  }

  .sig-presets {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    padding: 2px 6px;
  }

  .track-menu .sig-preset {
    padding: 2px 8px;
    border: 1px solid var(--border);
    font-size: 12px;
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

  /* 音声クリップのフェード(暗い三角)と、そのつまみ・音量のつまみ */
  .fade-shade {
    position: absolute;
    left: 0;
    top: 0;
    height: 100%;
    pointer-events: none;
    z-index: 1;
  }

  .fade-shade polygon {
    fill: rgba(0, 0, 0, 0.4);
  }

  .fade-h {
    position: absolute;
    bottom: 1px;
    width: 9px;
    height: 9px;
    margin-left: -1px;
    border-radius: 2px;
    background: rgba(255, 255, 255, 0.85);
    border: 1px solid rgba(0, 0, 0, 0.45);
    cursor: ew-resize;
    z-index: 3;
    opacity: 0;
    transition: opacity 0.1s;
  }

  .fade-h.out {
    margin-right: -1px;
  }

  .gain-h {
    position: absolute;
    bottom: 1px;
    left: 50%;
    transform: translateX(-50%);
    padding: 0 4px;
    border-radius: 3px;
    font-size: 10px;
    line-height: 13px;
    white-space: nowrap;
    color: #fff;
    background: rgba(0, 0, 0, 0.5);
    cursor: ns-resize;
    z-index: 3;
    opacity: 0;
    transition: opacity 0.1s;
  }

  /* つまみはクリップにマウスを乗せたときだけ出す(フェード・音量を変えてあれば薄く見せたまま) */
  .clip:hover .fade-h,
  .clip:hover .gain-h,
  .fade-h.active,
  .gain-h.active {
    opacity: 1;
  }

  .gain-h.set {
    opacity: 0.7;
  }

  .transcribe.poly {
    right: 32px;
  }

  .transcribe {
    position: absolute;
    top: 2px;
    right: 10px;
    z-index: 3;
    display: inline-flex;
    font-size: 11px;
    line-height: 1;
    padding: 2px 3px;
    border-radius: 4px;
    border: 1px solid rgba(255, 255, 255, 0.35);
    background: rgba(0, 0, 0, 0.35);
    color: #fff;
    cursor: pointer;
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

  .menu-backdrop {
    position: fixed;
    inset: 0;
    z-index: 19;
  }

  .track-menu {
    position: fixed;
    z-index: 20;
    /* 画面に収まらない分は中でスクロール(位置は keepInView が画面内へ寄せる) */
    max-height: calc(100vh - 16px);
    max-width: min(440px, calc(100vw - 16px));
    overflow-y: auto;
    background: var(--bg-panel);
    border: 1px solid var(--border-strong);
    border-radius: var(--r-lg);
    box-shadow: var(--shadow-pop);
    padding: 4px;
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 200px;
    font-size: var(--fs-md);
  }

  /* メニューの項目: アイコン + 文字 + 右端にキー */
  .track-menu button {
    display: flex;
    align-items: center;
    gap: 10px;
    text-align: left;
    border: none;
    background: none;
    padding: 6px 10px;
    border-radius: var(--r-sm);
    font-size: var(--fs-md);
    --icon-size: 15px;
  }

  .track-menu button > :global(.icon) {
    color: var(--text-dim);
  }

  .track-menu button.rich span {
    display: flex;
    flex-direction: column;
  }

  .track-menu button small {
    font-size: var(--fs-xs);
    color: var(--text-dim);
  }

  .ic-space {
    width: 15px;
    flex-shrink: 0;
  }

  .track-menu .key {
    margin-left: auto;
    padding-left: 24px;
    font-size: var(--fs-xs);
    font-family: var(--mono);
    color: var(--text-faint);
  }

  .track-menu button:hover:not(:disabled) {
    background: color-mix(in srgb, var(--accent) 15%, transparent);
  }

  .track-menu button.danger:hover {
    background: var(--danger-bg);
    color: var(--danger-text);
  }

  .menu-sep {
    height: 1px;
    background: var(--border);
    margin: 2px 4px;
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
