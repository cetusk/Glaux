<script lang="ts">
  import { open as pickFile } from "@tauri-apps/plugin-dialog";
  import * as api from "./api";
  import AutomationLaneRow from "./AutomationLaneRow.svelte";
  import { barAtTick, barsEndTick, buildBars } from "./barMap";
  import AudioClipPreview from "./AudioClipPreview.svelte";
  import ClipPreview from "./ClipPreview.svelte";
  import { newClipId, newNoteId, newTrackId } from "./ids";
  import {
    MASTER_FOCUS_ID,
    midiArmStore,
    pianoRollStore,
    selectionStore,
    soundDesignStore,
  } from "./selection.svelte";
  import type { Clip, PresetInfo, Project, Track } from "./types";

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

  // 4/4 の 1 小節 = 96px となる密度。小節の実幅は拍子に応じて変わる(7/8 は狭い)
  const PX_PER_WHOLE = 96;
  const pxPerTick = $derived(PX_PER_WHOLE / (project.ppq * 4));

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

  function volumeText(t: Track): string {
    const v = t.volume_db;
    return `${v > 0 ? "+" : ""}${v.toFixed(1)} dB`;
  }

  const HEAD_W = 200;
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

  let sigMenu = $state<{ x: number; y: number; barIndex: number; num: string; den: string } | null>(
    null,
  );
  const DENS = [1, 2, 4, 8, 16, 32];

  function openSigMenu(e: MouseEvent, barIndex: number) {
    e.preventDefault();
    const bar = barList[Math.min(barIndex, barList.length - 1)];
    sigMenu = {
      x: e.clientX,
      y: e.clientY,
      barIndex: bar.index,
      num: String(bar.num),
      den: String(bar.den),
    };
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

  /// 音声クリップ(単旋律)を譜起こしして MIDI クリップにし、ピアノロールで開く
  let transcribing = $state<string | null>(null);
  async function transcribe(track: Track, clip: Clip) {
    if (transcribing) return;
    transcribing = clip.id;
    try {
      const r = await api.transcribeClip(clip.id);
      pianoRollStore.focus = {
        clipId: r.clip_id,
        clipName: `${clip.name} (MIDI)`,
        trackId: r.track_id,
        trackName: r.created_track ? `${track.name} MIDI` : track.name,
        anchorTick: 0,
      };
    } catch (e) {
      alert(String(e));
    } finally {
      transcribing = null;
    }
  }

  /// 音声トラックの空きレーン: 音声ファイルを選んでその小節に音声クリップとして置く
  async function importAudioAt(track: Track, startTick: number) {
    const file = await pickFile({
      title: "音声ファイルをクリップとして配置",
      filters: [{ name: "音声(WAV / MP3 / FLAC / OGG / M4A)", extensions: ["wav", "mp3", "flac", "ogg", "m4a", "aac"] }],
    });
    if (typeof file !== "string") return;
    await api.importAudioClip(track.id, file, startTick).catch((e) => alert(`取り込めませんでした: ${e}`));
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
      const target = e.target as HTMLElement | null;
      if (target && (target.tagName === "TEXTAREA" || target.tagName === "INPUT")) return;
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
      | "follow-off",
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
      case "follow-off":
        setFollow(targets, false);
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

  function openTrackMenu(e: MouseEvent, track: Track, index: number) {
    e.preventDefault();
    trackMenu = { trackId: track.id, index, x: e.clientX, y: e.clientY };
  }

  function moveTrack(toIndex: number) {
    const menu = trackMenu;
    trackMenu = null;
    if (!menu || toIndex < 0 || toIndex >= project.tracks.length) return;
    const track = project.tracks[menu.index];
    api
      .applyEdit(
        [{ op: "move_track", id: menu.trackId, to_index: toIndex }],
        `${track?.name ?? "トラック"} を${toIndex < menu.index ? "上" : "下"}へ移動`,
      )
      .catch(() => {});
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

  const INSTRUMENTS = [
    { name: "subtractive", label: "🎹 subtractive", desc: "シンセ全般(リード・ベース・パッド)" },
    { name: "drum", label: "🥁 drum", desc: "ドラムシンセ(GM 配置、キット UI 対応)" },
    { name: "pluck", label: "🎸 pluck", desc: "撥弦モデル(ギター・ベース・ハープ)" },
  ];

  let deviceMenu = $state<{ track: Track; x: number; y: number } | null>(null);
  let presets = $state<PresetInfo[]>([]);
  let presetName = $state("");
  let presetMsg = $state<string | null>(null);

  function openDeviceMenu(e: MouseEvent, track: Track) {
    e.stopPropagation();
    presetMsg = null;
    presetName = "";
    deviceMenu = { track, x: e.clientX, y: e.clientY };
    api
      .listPresets()
      .then((r) => (presets = r.presets))
      .catch(() => (presets = []));
  }

  function setDevice(name: string) {
    const menu = deviceMenu;
    deviceMenu = null;
    if (!menu) return;
    if (menu.track.device?.name === name) return; // 変更なし
    api
      .applyEdit(
        [
          {
            op: "set_device",
            track: menu.track.id,
            device: { type: "builtin", name },
          },
        ],
        `${menu.track.name} の音源を ${name} に変更`,
      )
      .catch(() => {});
  }

  function applyPreset(name: string) {
    const menu = deviceMenu;
    deviceMenu = null;
    if (!menu) return;
    api.loadPreset(menu.track.id, name).catch((e) => {
      console.error(e);
    });
  }

  async function saveCurrentPreset() {
    const menu = deviceMenu;
    const name = presetName.trim();
    if (!menu || !name) return;
    try {
      await api.savePreset(menu.track.id, name);
      presetMsg = `保存しました: ${name}`;
      presetName = "";
      const r = await api.listPresets();
      presets = r.presets;
    } catch (e) {
      presetMsg = String(e);
    }
  }

  function addTrack(kind: "midi" | "audio" = "midi") {
    const id = newTrackId();
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

  function toggleSolo(t: Track) {
    api
      .applyEdit(
        [{ op: "set_track_prop", id: t.id, prop: "solo", value: !t.solo }],
        `${t.name} のソロを${t.solo ? "解除" : "オン"}`,
      )
      .catch(() => {});
  }
</script>

<div class="timeline" bind:this={root}>
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
          <div
            class="section-band"
            style="left:{sec.tick * pxPerTick}px;{next !== undefined
              ? `width:${(next - sec.tick) * pxPerTick}px`
              : `right:0`}"
            title={`${sec.name}(tick ${sec.tick}〜)`}
          >
            {sec.name}
          </div>
        {/each}
      </div>
    </div>
  {/if}

  <!-- 小節ルーラー(クリックでシーク、ドラッグで範囲選択) -->
  <div class="ruler-row">
    <div class="track-head ruler-head"></div>
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
        <div class="bar-mark" style="left:{bar.tick * pxPerTick}px">
          {bar.index + 1}{#if bar.sigChange}<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions --><span
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

  {#if project.tracks.length === 0}
    <div class="empty">
      トラックがありません。Claude に「トラックを追加して」と頼んでみてください。
    </div>
  {/if}

  {#each project.tracks as track, ti (track.id)}
    <div class="track-row" class:alt={ti % 2 === 1}>
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="track-head" oncontextmenu={(e) => openTrackMenu(e, track, ti)}>
        <div class="head-row">
          <div class="track-name" style={track.color ? `color:${track.color}` : ""}>
            {track.name}
          </div>
          <button class="ms" class:mute-on={track.mute} onclick={() => toggleMute(track)} title="ミュート">
            M
          </button>
          <button class="ms" class:solo-on={track.solo} onclick={() => toggleSolo(track)} title="ソロ">
            S
          </button>
          {#if track.kind === "midi"}
            <button
              class="ms"
              class:arm-on={midiArmStore.trackId === track.id}
              onclick={() =>
                (midiArmStore.trackId = midiArmStore.trackId === track.id ? null : track.id)}
              title="MIDI キーボードでこのトラックを弾く(ON の間は ⏺ が MIDI 録音になります)"
            >
              🎹
            </button>
          {/if}
          <button
            class="ms"
            class:auto-on={autoLanes[track.id] !== undefined}
            onclick={() => toggleAutoLane(track.id)}
            title="オートメーションレーンを開閉"
          >
            〜
          </button>
          <button
            class="ms"
            class:auto-on={soundDesignStore.focus?.trackId === track.id}
            onclick={() =>
              (soundDesignStore.focus =
                soundDesignStore.focus?.trackId === track.id
                  ? null
                  : { trackId: track.id, trackName: track.name })}
            title="音作りビューを開閉(つまみ・エフェクト・プリセット)"
          >
            🎛
          </button>
        </div>
        <div class="head-row">
          <input
            class="vol"
            type="range"
            min="-40"
            max="6"
            step="0.5"
            value={track.volume_db}
            disabled={hasVolumeLane(track)}
            title={hasVolumeLane(track)
              ? "音量オートメーション使用中(フェーダーより優先されます)"
              : ""}
            onchange={(e) => setVolume(track, e)}
          />
          <span class="db">{volumeText(track)}</span>
        </div>
        <div class="track-meta">
          <span class="kind {track.kind}">{track.kind}</span>
          <button
            class="dev"
            onclick={(e) => openDeviceMenu(e, track)}
            title={track.device?.name
              ? "クリックで音源を変更"
              : "音源未設定(既定の subtractive で発音)。クリックで選択"}
          >
            🎹 {track.device?.name ?? "subtractive*"} ▾
          </button>
          <code>{track.id}</code>
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
        title={track.kind === "audio"
          ? "ダブルクリックで音声ファイル(WAV / MP3 等)をその小節に配置(録音は ⏺ ボタン)"
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
              >{clip.kind === "audio" ? "🎵 " : clip.loop && clip.loop_len ? "🔁 " : ""}{clip.name}{followBpm(
                clip,
              ) !== null
                ? ` ⇔${followBpm(clip)}`
                : ""}</span
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
              <button
                class="transcribe"
                disabled={transcribing !== null}
                title="譜起こし: この音声(鼻歌・歌・単音)を MIDI クリップにする(単旋律のみ)"
                onpointerdown={(e) => e.stopPropagation()}
                ondblclick={(e) => e.stopPropagation()}
                onclick={(e) => {
                  e.stopPropagation();
                  transcribe(track, clip);
                }}
              >
                {transcribing === clip.id ? "…" : "♪"}
              </button>
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
  {/each}

  <!-- マスター: 曲全体の音量・マスターのエフェクトのオートメーション -->
  <div class="track-row master-row">
    <div class="track-head">
      <div class="head-row">
        <div class="track-name">マスター</div>
        <button
          class="ms"
          class:auto-on={autoLanes[MASTER_FOCUS_ID] !== undefined}
          onclick={() => toggleAutoLane(MASTER_FOCUS_ID)}
          title="マスターのオートメーション(曲全体のフェードアウト、マスターのエフェクトの時間変化)"
        >
          〜{masterLaneCount > 0 ? ` ${masterLaneCount}` : ""}
        </button>
      </div>
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

  {#if sigMenu}
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="menu-backdrop" onclick={() => (sigMenu = null)} oncontextmenu={(e) => { e.preventDefault(); sigMenu = null; }}></div>
    <div class="track-menu sig-menu" style="left:{sigMenu.x}px;top:{sigMenu.y}px">
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
        <button class="danger" onclick={removeSig}>🗑 この拍子変更を削除(前の拍子に戻す)</button>
      {/if}
      <div class="menu-note">ノートの位置は変わらず、この小節から先の小節線だけが変わります(Ctrl+Z で戻せます)</div>
    </div>
  {/if}

  {#if clipMenu}
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="menu-backdrop" onclick={() => (clipMenu = null)} oncontextmenu={(e) => { e.preventDefault(); clipMenu = null; }}></div>
    <div class="track-menu clip-menu" style="left:{clipMenu.x}px;top:{clipMenu.y}px">
      {#if selectedClips.size > 1}
        <div class="menu-note">選択中のクリップ {selectedClips.size} 個が対象</div>
      {/if}
      {#if clipMenu.clip.kind === "midi"}
        {#if clipMenu.clip.loop && clipMenu.clip.loop_len}
          <button onclick={() => menuAction("loop-off")}>🔁 ループを解除</button>
          <button onclick={() => menuAction("expand")}>🔁 繰り返しをノートに展開(個別に編集できるように)</button>
        {:else}
          <button onclick={() => menuAction("loop-on")}>🔁 ループにする(今の長さを繰り返す。右端を伸ばすと繰り返し)</button>
        {/if}
        <div class="menu-sep"></div>
      {:else}
        {#if followBpm(clipMenu.clip) !== null}
          <button onclick={() => menuAction("follow-off")}>
            ⇔ テンポ追従を解除(今は {followBpm(clipMenu.clip)} BPM の素材として伸縮中)
          </button>
        {:else}
          <button onclick={() => menuAction("follow-on")}>
            ⇔ テンポに追従させる({bpmAt(clipMenu.clip.start)} BPM で録った素材として、テンポを変えても拍がずれないよう伸縮)
          </button>
        {/if}
        <div class="menu-sep"></div>
      {/if}
      <button onclick={() => menuAction("split")}>✂ ここで分割({barLabel(clipMenu.at)})</button>
      <button onclick={() => menuAction("split-head")}>✂ 再生ヘッドで分割 <span class="key">S</span></button>
      <div class="menu-sep"></div>
      <button onclick={() => menuAction("dup")}>⧉ 複製(直後に並べる) <span class="key">Ctrl+D</span></button>
      <button onclick={() => menuAction("copy")}>コピー <span class="key">Ctrl+C</span></button>
      <button onclick={() => menuAction("cut")}>切り取り <span class="key">Ctrl+X</span></button>
      <div class="menu-sep"></div>
      <button class="danger" onclick={() => menuAction("delete")}>🗑 削除 <span class="key">Delete</span></button>
      <div class="menu-note">貼り付け(Ctrl+V)は再生ヘッドの位置・元のトラックに置かれます</div>
    </div>
  {/if}

  {#if deviceMenu}
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="menu-backdrop" onclick={() => (deviceMenu = null)} oncontextmenu={(e) => { e.preventDefault(); deviceMenu = null; }}></div>
    <div class="track-menu" style="left:{deviceMenu.x}px;top:{deviceMenu.y}px">
      {#each INSTRUMENTS as inst (inst.name)}
        <button
          class:active-dev={(deviceMenu.track.device?.name ?? "subtractive") === inst.name}
          onclick={() => setDevice(inst.name)}
        >
          <span class="dev-label">
            {inst.label}{(deviceMenu.track.device?.name ?? "subtractive") === inst.name ? " ✓" : ""}
          </span>
          <span class="dev-desc">{inst.desc}</span>
        </button>
      {/each}
      <div class="menu-note">切り替えると音源パラメータは初期値に戻ります(Ctrl+Z で取り消せます)。細かい音作りは AI に依頼してください。</div>

      <div class="menu-sep"></div>
      <div class="preset-title">プリセット(全プロジェクト共通)</div>
      {#if presets.length === 0}
        <div class="menu-note">まだありません。良い音ができたら下の欄で保存できます。</div>
      {:else}
        {#each presets as p (p.name)}
          <button
            onclick={() => applyPreset(p.name)}
            title={`${p.description ?? ""}\n音源: ${p.instrument}${p.effects.length ? " / FX: " + p.effects.join(" → ") : ""}\n適用すると音源とエフェクトが置き換わります(Ctrl+Z 可)`}
          >
            <span class="dev-label">🎨 {p.name}</span>
            <span class="dev-desc">{p.instrument}{p.effects.length ? ` + ${p.effects.join(", ")}` : ""}</span>
          </button>
        {/each}
      {/if}
      <div class="preset-save">
        <input
          type="text"
          placeholder="今の音を保存(名前)"
          bind:value={presetName}
          onkeydown={(e) => {
            if (e.key === "Enter" && !e.isComposing) {
              e.preventDefault();
              saveCurrentPreset();
            }
          }}
          onclick={(e) => e.stopPropagation()}
        />
        <button class="save-btn" disabled={!presetName.trim()} onclick={saveCurrentPreset}>保存</button>
      </div>
      {#if presetMsg}
        <div class="menu-note">{presetMsg}</div>
      {/if}
    </div>
  {/if}

  {#if trackMenu}
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="menu-backdrop" onclick={() => (trackMenu = null)} oncontextmenu={(e) => { e.preventDefault(); trackMenu = null; }}></div>
    <div class="track-menu" style="left:{trackMenu.x}px;top:{trackMenu.y}px">
      <button disabled={trackMenu.index === 0} onclick={() => moveTrack(trackMenu!.index - 1)}>
        ↑ 上へ移動
      </button>
      <button
        disabled={trackMenu.index >= project.tracks.length - 1}
        onclick={() => moveTrack(trackMenu!.index + 1)}
      >
        ↓ 下へ移動
      </button>
      <div class="menu-sep"></div>
      <button class="danger" onclick={deleteTrack} title="Ctrl+Z で元に戻せます">
        🗑 トラックを削除
      </button>
    </div>
  {/if}

  <div class="add-track-row">
    <button class="add-track" onclick={() => addTrack("midi")} title="MIDI トラックを追加(音源は後から AI に頼むか自動で subtractive)">
      + トラックを追加
    </button>
    <button class="add-track" onclick={() => addTrack("audio")} title="音声トラックを追加(音声ファイルの配置・録音先。空きレーンをダブルクリックで WAV / MP3 等を配置)">
      + 🎵 音声トラック
    </button>
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

  .ruler-row,
  .track-row {
    display: flex;
    border-bottom: 1px solid var(--border);
  }

  .track-head {
    width: 200px;
    flex-shrink: 0;
    padding: 6px 10px;
    background: var(--bg-panel);
    border-right: 1px solid var(--border);
    position: sticky;
    left: 0;
    z-index: 2;
  }

  .head-row {
    display: flex;
    align-items: center;
    gap: 5px;
  }

  .ms {
    padding: 0 6px;
    font-size: 10px;
    font-weight: 700;
    line-height: 16px;
    border-radius: 3px;
  }

  .ms.mute-on {
    background: #6b4030;
    border-color: #8a5a44;
    color: #ffab7a;
  }

  .ms.solo-on {
    background: #6b6130;
    border-color: #8a7d44;
    color: var(--accent);
  }

  .ms.arm-on {
    background: #6b3030;
    border-color: #a04848;
  }

  .ms.auto-on {
    border-color: var(--accent-dim);
    color: var(--accent);
  }

  .vol {
    flex: 1;
    min-width: 0;
    accent-color: var(--accent);
    height: 14px;
  }

  .db {
    font-size: 10px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
    width: 52px;
    text-align: right;
  }

  .ruler-head {
    padding: 0;
    height: 24px;
  }

  .section-row {
    display: flex;
    border-bottom: 1px solid var(--border);
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
    pointer-events: none;
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
    height: 72px;
  }

  .track-row .lane {
    position: relative;
    background: var(--bg-lane);
  }

  .track-row.master-row {
    height: 30px;
    border-top: 2px solid var(--border);
  }

  .master-row .track-head {
    padding: 5px 10px;
  }

  .master-row .track-name {
    color: var(--text-dim);
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

  .track-meta {
    display: flex;
    gap: 6px;
    align-items: center;
    font-size: 10px;
    color: var(--text-dim);
    margin-top: 2px;
    opacity: 0.8;
  }

  .dev {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    border: none;
    background: none;
    padding: 0;
    font-size: 10px;
    color: var(--text-dim);
    cursor: pointer;
  }

  .dev:hover {
    color: var(--accent);
  }

  .dev-label {
    display: block;
    font-size: 12px;
    font-weight: 600;
  }

  .dev-desc {
    display: block;
    font-size: 10px;
    color: var(--text-dim);
  }

  .track-menu button.active-dev {
    border: 1px solid var(--accent-dim);
  }

  .menu-note {
    font-size: 9px;
    color: var(--text-dim);
    padding: 4px 10px 2px;
    max-width: 230px;
    line-height: 1.5;
  }

  .preset-title {
    font-size: 10px;
    color: var(--text-dim);
    padding: 2px 10px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .preset-save {
    display: flex;
    gap: 4px;
    padding: 4px 8px 2px;
  }

  .preset-save input {
    flex: 1;
    min-width: 0;
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 3px 6px;
    font-size: 11px;
  }

  .preset-save input:focus {
    outline: none;
    border-color: var(--accent-dim);
  }

  .preset-save .save-btn {
    font-size: 11px;
    padding: 2px 8px;
  }

  .kind {
    padding: 0 5px;
    border-radius: 3px;
    font-size: 10px;
    text-transform: uppercase;
  }

  .kind.midi {
    background: color-mix(in srgb, var(--clip-midi) 30%, transparent);
    color: var(--clip-midi);
  }

  .kind.audio {
    background: color-mix(in srgb, var(--clip-audio) 30%, transparent);
    color: var(--clip-audio);
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

  .clip-menu .key {
    float: right;
    margin-left: 16px;
    font-size: 10px;
    color: var(--text-dim);
  }

  .clip-menu .danger,
  .sig-menu .danger {
    color: #e8a07c;
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

  .transcribe {
    position: absolute;
    top: 2px;
    right: 10px;
    z-index: 3;
    font-size: 11px;
    line-height: 1;
    padding: 1px 5px;
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
    font-size: 11px;
    white-space: nowrap;
    position: relative;
    z-index: 1;
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
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);
    padding: 5px;
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 160px;
  }

  .track-menu button {
    text-align: left;
    border: none;
    background: none;
    padding: 6px 10px;
    border-radius: 5px;
  }

  .track-menu button:hover:not(:disabled) {
    background: color-mix(in srgb, var(--accent) 15%, transparent);
  }

  .track-menu button.danger:hover {
    background: #5c2b33;
    color: #ffb4c0;
  }

  .menu-sep {
    height: 1px;
    background: var(--border);
    margin: 2px 4px;
  }

  .add-track-row {
    padding: 8px 10px;
    position: sticky;
    left: 0;
    width: 200px;
  }

  .add-track {
    width: 100%;
    color: var(--text-dim);
    border-style: dashed;
  }

  .add-track:hover {
    color: var(--accent);
  }
</style>
