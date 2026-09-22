<script lang="ts">
  import { onMount, tick as sveltick } from "svelte";
  import * as api from "./api";
  import DrumKit from "./DrumKit.svelte";
  import { drumName } from "./drumMap";
  import { newNoteId } from "./ids";
  import { pianoRollStore } from "./selection.svelte";
  import type { MidiClip, Note, Project, Track } from "./types";

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

  // ---- 対象クリップ(プロジェクト更新のたびに再導出 = AI の編集がライブ反映) ----

  const found = $derived.by((): { track: Track; clip: MidiClip } | null => {
    const focus = pianoRollStore.focus;
    if (!focus) return null;
    for (const track of project.tracks) {
      for (const clip of track.clips) {
        if (clip.id === focus.clipId && clip.kind === "midi") {
          return { track, clip };
        }
      }
    }
    return null;
  });

  // クリップが消えた(AI が削除した等)ら閉じる
  $effect(() => {
    if (pianoRollStore.focus && !found) {
      pianoRollStore.focus = null;
    }
  });

  function close() {
    pianoRollStore.focus = null;
  }

  // ---- 座標系 ----

  const KEY_W = 52;
  const RULER_H = 22;
  // ズーム(Ctrl+ホイール: 横、Shift+ホイール: 縦)
  let pxPerBeat = $state(60);
  let rowH = $state(14);
  const pxPerTick = $derived(pxPerBeat / 960);

  const clip = $derived(found?.clip ?? null);
  const contentW = $derived(clip ? Math.max(clip.length * pxPerTick, 200) : 200);
  const contentH = $derived(128 * rowH);

  const isDrum = $derived(found?.track.device?.name === "drum");
  /// 挿入カーソル(クリックで固定。ドラム打ち込み先。←/→ でスナップ移動)
  let insertTick = $state(0);
  let showKit = $state(true);
  let drumHighlight = $state<number | null>(null);

  let snapTicks = $state(480); // 1/8
  const snapOptions = [
    { label: "1 小節", ticks: 3840 },
    { label: "1/2", ticks: 1920 },
    { label: "1/4", ticks: 960 },
    { label: "1/8", ticks: 480 },
    { label: "1/16", ticks: 240 },
  ];

  const NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
  const BLACK = new Set([1, 3, 6, 8, 10]);

  function noteName(pitch: number): string {
    return `${NOTE_NAMES[pitch % 12]}${Math.floor(pitch / 12) - 1}`;
  }

  // ---- 選択・ドラッグ状態 ----

  let selected = $state<Set<string>>(new Set());

  type Drag =
    | {
        mode: "move";
        startTick: number;
        startPitch: number;
        basePitch: number;
        dt: number;
        dp: number;
        ids: string[];
      }
    | { mode: "resize"; noteId: string; startTick: number; dt: number }
    | { mode: "select"; x0: number; y0: number; x1: number; y1: number };
  let drag = $state<Drag | null>(null);

  // 2 層 Canvas: base = グリッド + ノート(内容が変わったときだけ再描画)、
  // overlay = 再生ヘッド・カーソル・ドラッグゴースト(高頻度更新はこちらだけ)
  let canvasEl: HTMLCanvasElement | undefined = $state();
  let overlayEl: HTMLCanvasElement | undefined = $state();
  let scroller: HTMLDivElement | undefined = $state();

  // ブラウザの Canvas 実サイズ上限(超えると描画が黙って全部消える)。
  // 長いクリップ × ズームで超えうるので、上限内に収まる解像度スケールに落とす
  // (見た目は CSS サイズのまま。極端な場合だけ少しぼやける)
  const MAX_CANVAS_PX = 15000;

  function ensureSize(c: HTMLCanvasElement): CanvasRenderingContext2D {
    const dpr = window.devicePixelRatio || 1;
    const scale = Math.min(dpr, MAX_CANVAS_PX / contentW, MAX_CANVAS_PX / contentH);
    const w = Math.max(1, Math.round(contentW * scale));
    const h = Math.max(1, Math.round(contentH * scale));
    if (c.width !== w || c.height !== h) {
      c.width = w;
      c.height = h;
    }
    const g = c.getContext("2d")!;
    g.setTransform(scale, 0, 0, scale, 0, 0);
    return g;
  }

  // ---- 描画 ----

  function drawBase() {
    const c = canvasEl;
    const currentClip = clip;
    if (!c || !currentClip) return;
    const g = ensureSize(c);
    g.clearRect(0, 0, contentW, contentH);

    // 行の縞(黒鍵行を暗く)
    for (let pitch = 0; pitch < 128; pitch++) {
      const y = (127 - pitch) * rowH;
      g.fillStyle = BLACK.has(pitch % 12) ? "#201d31" : "#262339";
      g.fillRect(0, y, contentW, rowH);
      if (pitch % 12 === 0) {
        // C の行の下線を強調
        g.fillStyle = "#3a3654";
        g.fillRect(0, y + rowH - 1, contentW, 1);
      }
    }

    // ドラムパーツ選択中の行をハイライト
    if (drumHighlight !== null) {
      g.fillStyle = "rgba(255, 194, 71, 0.08)";
      g.fillRect(0, (127 - drumHighlight) * rowH, contentW, rowH);
    }

    // 拍・小節線
    for (let t = 0; t <= currentClip.length; t += 960) {
      const x = t * pxPerTick;
      const isBar = t % 3840 === 0;
      g.fillStyle = isBar ? "#4a4568" : "#332f4c";
      g.fillRect(x, 0, 1, contentH);
    }
    // スナップグリッド(拍より細かいときだけ)
    if (snapTicks < 960) {
      g.fillStyle = "#2b2841";
      for (let t = 0; t <= currentClip.length; t += snapTicks) {
        if (t % 960 !== 0) g.fillRect(t * pxPerTick, 0, 1, contentH);
      }
    }

    // ノート(素の位置。ドラッグ中のゴーストはオーバーレイ側で描く)
    for (const n of currentClip.notes) {
      const x = n.pos * pxPerTick;
      const y = (127 - n.pitch) * rowH;
      const w = Math.max(n.dur * pxPerTick, 4);
      const isSel = selected.has(n.id);
      const alpha = 0.45 + (n.vel / 127) * 0.55;
      g.fillStyle = isSel ? `rgba(255, 194, 71, ${alpha})` : `rgba(94, 156, 224, ${alpha})`;
      g.beginPath();
      g.roundRect(x, y + 1.5, w, rowH - 3, 3);
      g.fill();
      g.strokeStyle = isSel ? "#ffd98a" : "rgba(255,255,255,0.25)";
      g.lineWidth = 1;
      g.stroke();
    }
  }

  function drawOverlay() {
    const c = overlayEl;
    const currentClip = clip;
    if (!c || !currentClip) return;
    const g = ensureSize(c);
    g.clearRect(0, 0, contentW, contentH);

    // ドラッグ中の移動/リサイズゴースト
    if (drag?.mode === "move" && (drag.dt !== 0 || drag.dp !== 0)) {
      const ids = new Set(drag.ids);
      g.strokeStyle = "#ffd98a";
      g.fillStyle = "rgba(255, 194, 71, 0.35)";
      g.lineWidth = 1;
      for (const n of currentClip.notes) {
        if (!ids.has(n.id)) continue;
        const pos = Math.max(0, Math.min(currentClip.length - 1, n.pos + drag.dt));
        const pitch = Math.max(0, Math.min(127, n.pitch + drag.dp));
        g.beginPath();
        g.roundRect(pos * pxPerTick, (127 - pitch) * rowH + 1.5, Math.max(n.dur * pxPerTick, 4), rowH - 3, 3);
        g.fill();
        g.stroke();
      }
    }
    if (drag?.mode === "resize" && drag.dt !== 0) {
      const n = currentClip.notes.find((note) => note.id === drag.noteId);
      if (n) {
        const dur = Math.max(60, n.dur + drag.dt);
        g.strokeStyle = "#ffd98a";
        g.fillStyle = "rgba(255, 194, 71, 0.35)";
        g.beginPath();
        g.roundRect(n.pos * pxPerTick, (127 - n.pitch) * rowH + 1.5, Math.max(dur * pxPerTick, 4), rowH - 3, 3);
        g.fill();
        g.stroke();
      }
    }

    // 矩形選択
    if (drag?.mode === "select") {
      const x = Math.min(drag.x0, drag.x1);
      const y = Math.min(drag.y0, drag.y1);
      const w = Math.abs(drag.x1 - drag.x0);
      const h = Math.abs(drag.y1 - drag.y0);
      g.fillStyle = "rgba(255, 194, 71, 0.08)";
      g.fillRect(x, y, w, h);
      g.strokeStyle = "rgba(255, 194, 71, 0.6)";
      g.strokeRect(x, y, w, h);
    }

    // 挿入カーソル(クリックで固定。ドラム打ち込み先)
    if (insertTick <= currentClip.length) {
      const ix = insertTick * pxPerTick;
      g.fillStyle = "#5da2e8";
      g.fillRect(ix, 0, 1.5, contentH);
      g.beginPath();
      g.moveTo(ix - 5, 0);
      g.lineTo(ix + 6.5, 0);
      g.lineTo(ix + 0.75, 8);
      g.closePath();
      g.fill();
    }

    // ホバー位置(薄い線。Ctrl+V の貼り付け先の目印)
    if (hoverSnapTick <= currentClip.length) {
      const hx = hoverSnapTick * pxPerTick;
      g.fillStyle = "rgba(255, 194, 71, 0.28)";
      g.fillRect(hx, 0, 1, contentH);
    }

    // 再生ヘッド(クリップ内にあるときだけ)
    const rel = playheadTick - currentClip.start;
    if (rel >= 0 && rel <= currentClip.length) {
      g.fillStyle = "#ffc247";
      g.fillRect(rel * pxPerTick, 0, 1.5, contentH);
    }
  }

  // 静的層: 内容・選択・ズーム・スナップが変わったときだけ
  $effect(() => {
    void clip;
    void selected;
    void snapTicks;
    void drumHighlight;
    void pxPerBeat;
    void rowH;
    drawBase();
  });

  // 動的層: 高頻度更新(再生ヘッド・カーソル・ドラッグ)はこちらだけ再描画
  $effect(() => {
    void clip;
    void drag;
    void playheadTick;
    void insertTick;
    void hoverSnapTick;
    void pxPerBeat;
    void rowH;
    void playing;
    drawOverlay();
    followPlayhead();
  });

  // 再生ヘッドの位置が変わったら追従スクロール(ページ送り型)。
  // 再生中はもちろん、停止中のシーク(ルーラー・⏪⏩・Home/End)でも追従する。
  let lastFollowTick: number | null = null;

  function followPlayhead() {
    const currentClip = clip;
    if (!scroller || !currentClip) return;
    if (playheadTick === lastFollowTick) return;
    const first = lastFollowTick === null;
    lastFollowTick = playheadTick;
    if (first) return; // 開いた直後はアンカー位置へのスクロールを優先
    const rel = playheadTick - currentClip.start;
    if (rel < 0 || rel > currentClip.length) return;
    const px = KEY_W + rel * pxPerTick;
    const view = scroller.clientWidth;
    const left = scroller.scrollLeft;
    if (px > left + view - 80 || px < left + KEY_W) {
      scroller.scrollLeft = Math.max(0, px - KEY_W - 80);
    }
  }

  // 開いたときにノートのある高さへスクロール
  $effect(() => {
    const focus = pianoRollStore.focus;
    if (!focus) return;
    lastFollowTick = null;
    sveltick().then(() => {
      const currentClip = clip;
      if (!scroller || !currentClip) return;
      const pitches = currentClip.notes.map((n) => n.pitch);
      const center = pitches.length
        ? (Math.min(...pitches) + Math.max(...pitches)) / 2
        : 60;
      scroller.scrollTop = Math.max(0, (127 - center) * rowH - scroller.clientHeight / 2);
      const anchor = focus.anchorTick ?? 0;
      insertTick = Math.max(
        0,
        Math.min(snapFloor(anchor), currentClip.length - 60),
      );
      scroller.scrollLeft = Math.max(0, anchor * pxPerTick - scroller.clientWidth / 2);
    });
  });

  // ---- ズーム(Ctrl+ホイール: 横、Shift+ホイール: 縦)。カーソル位置を維持 ----
  $effect(() => {
    const el = scroller;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (e.ctrlKey) {
        e.preventDefault();
        const rect = el.getBoundingClientRect();
        const cx = e.clientX - rect.left;
        const tickAt = (el.scrollLeft + cx - KEY_W) / (pxPerBeat / 960);
        const next = Math.min(240, Math.max(12, pxPerBeat * (e.deltaY < 0 ? 1.25 : 0.8)));
        if (next === pxPerBeat) return;
        pxPerBeat = next;
        sveltick().then(() => {
          el.scrollLeft = Math.max(0, tickAt * (next / 960) - (cx - KEY_W));
        });
      } else if (e.shiftKey) {
        e.preventDefault();
        const rect = el.getBoundingClientRect();
        const cy = e.clientY - rect.top;
        const rowAt = (el.scrollTop + cy - RULER_H) / rowH;
        const next = Math.min(30, Math.max(7, rowH + (e.deltaY < 0 ? 2 : -2)));
        if (next === rowH) return;
        rowH = next;
        sveltick().then(() => {
          el.scrollTop = Math.max(0, rowAt * next - (cy - RULER_H));
        });
      }
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  });

  // ---- ヒットテスト・編集 ----

  function snapFloor(t: number): number {
    return Math.floor(t / snapTicks) * snapTicks;
  }

  function noteAt(x: number, y: number): Note | null {
    const currentClip = clip;
    if (!currentClip) return null;
    const pitch = 127 - Math.floor(y / rowH);
    const tick = x / pxPerTick;
    // 後勝ち(上に描かれるもの)を優先するため逆順
    for (let i = currentClip.notes.length - 1; i >= 0; i--) {
      const n = currentClip.notes[i];
      if (n.pitch === pitch && tick >= n.pos && tick <= n.pos + Math.max(n.dur, 4 / pxPerTick)) {
        return n;
      }
    }
    return null;
  }

  function nearRightEdge(n: Note, x: number): boolean {
    const right = (n.pos + n.dur) * pxPerTick;
    return right - x < 6 && right - x > -3;
  }

  function preview(pitch: number) {
    const trackId = found?.track.id;
    if (trackId) api.previewNote(trackId, pitch).catch(() => {});
  }

  async function applyEdit(commands: unknown[], label: string) {
    try {
      await api.applyEdit(commands, label);
    } catch (e) {
      console.error(e);
    }
  }

  function onPointerDown(e: PointerEvent) {
    if (e.button === 2) return; // 右クリックは contextmenu で処理
    const currentClip = clip;
    if (!currentClip) return;
    const x = e.offsetX;
    const y = e.offsetY;
    const hit = noteAt(x, y);
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);

    if (hit) {
      if (e.shiftKey) {
        const next = new Set(selected);
        if (next.has(hit.id)) next.delete(hit.id);
        else next.add(hit.id);
        selected = next;
        return;
      }
      if (!selected.has(hit.id)) {
        selected = new Set([hit.id]);
      }
      if (nearRightEdge(hit, x)) {
        drag = { mode: "resize", noteId: hit.id, startTick: x / pxPerTick, dt: 0 };
      } else {
        drag = {
          mode: "move",
          startTick: x / pxPerTick,
          startPitch: 127 - Math.floor(y / rowH),
          basePitch: hit.pitch,
          dt: 0,
          dp: 0,
          ids: [...(selected.has(hit.id) ? selected : new Set([hit.id]))],
        };
        preview(hit.pitch);
      }
    } else {
      drag = { mode: "select", x0: x, y0: y, x1: x, y1: y };
    }
  }

  // 貼り付け位置・カーソル形状用のホバー位置(canvas の絶対座標基準で追跡)
  let hoverTick = $state(0);
  const hoverSnapTick = $derived.by(() => {
    const t = Math.floor(hoverTick / snapTicks) * snapTicks;
    return Math.max(0, t);
  });

  function updateHover(e: PointerEvent) {
    if (!canvasEl) return;
    const rect = canvasEl.getBoundingClientRect();
    hoverTick = Math.max(0, (e.clientX - rect.left) / pxPerTick);
  }

  function onPointerMove(e: PointerEvent) {
    updateHover(e);
    if (!drag) {
      // カーソル形状
      const hit = noteAt(e.offsetX, e.offsetY);
      const el = e.currentTarget as HTMLElement;
      el.style.cursor = hit ? (nearRightEdge(hit, e.offsetX) ? "ew-resize" : "move") : "default";
      return;
    }
    const x = e.offsetX;
    const y = e.offsetY;
    if (drag.mode === "move") {
      const rawDt = x / pxPerTick - drag.startTick;
      const dp = (127 - Math.floor(y / rowH)) - drag.startPitch;
      if (dp !== drag.dp) {
        preview(Math.max(0, Math.min(127, drag.basePitch + dp)));
      }
      drag = {
        ...drag,
        dt: Math.round(rawDt / snapTicks) * snapTicks,
        dp,
      };
    } else if (drag.mode === "resize") {
      const rawDt = x / pxPerTick - drag.startTick;
      drag = { ...drag, dt: Math.round(rawDt / snapTicks) * snapTicks };
    } else {
      drag = { ...drag, x1: x, y1: y };
    }
  }

  function onPointerUp() {
    const currentClip = clip;
    if (!drag || !currentClip) {
      drag = null;
      return;
    }
    const d = drag;
    drag = null;

    if (d.mode === "select") {
      const x0 = Math.min(d.x0, d.x1);
      const x1 = Math.max(d.x0, d.x1);
      const y0 = Math.min(d.y0, d.y1);
      const y1 = Math.max(d.y0, d.y1);
      if (x1 - x0 < 3 && y1 - y0 < 3) {
        // ただのクリック = 選択解除 + 挿入カーソルをここに固定
        selected = new Set();
        insertTick = Math.max(
          0,
          Math.min(snapFloor(d.x0 / pxPerTick), currentClip.length - 60),
        );
        return;
      }
      const next = new Set<string>();
      for (const n of currentClip.notes) {
        const nx0 = n.pos * pxPerTick;
        const nx1 = (n.pos + n.dur) * pxPerTick;
        const ny = (127 - n.pitch) * rowH + rowH / 2;
        if (nx1 >= x0 && nx0 <= x1 && ny >= y0 && ny <= y1) next.add(n.id);
      }
      selected = next;
      return;
    }

    if (d.mode === "move") {
      if (d.dt === 0 && d.dp === 0) return;
      const changes = currentClip.notes
        .filter((n) => d.ids.includes(n.id))
        .map((n) => ({
          id: n.id,
          pos: Math.max(0, Math.min(currentClip.length - 1, n.pos + d.dt)),
          pitch: Math.max(0, Math.min(127, n.pitch + d.dp)),
        }));
      if (changes.length > 0) {
        applyEdit(
          [{ op: "update_notes", clip: currentClip.id, changes }],
          `ノートを移動(${changes.length} 個)`,
        );
      }
      return;
    }

    if (d.mode === "resize" && d.dt !== 0) {
      const n = currentClip.notes.find((note) => note.id === d.noteId);
      if (!n) return;
      const dur = Math.max(60, Math.min(currentClip.length - n.pos, n.dur + d.dt));
      applyEdit(
        [{ op: "update_notes", clip: currentClip.id, changes: [{ id: n.id, dur }] }],
        "ノートの長さを変更",
      );
    }
  }

  function onDblClick(e: MouseEvent) {
    const currentClip = clip;
    if (!currentClip) return;
    if (noteAt(e.offsetX, e.offsetY)) return;
    const pos = Math.max(0, Math.min(snapFloor(e.offsetX / pxPerTick), currentClip.length - 60));
    const pitch = Math.max(0, Math.min(127, 127 - Math.floor(e.offsetY / rowH)));
    const dur = Math.min(snapTicks, currentClip.length - pos);
    const id = newNoteId();
    selected = new Set([id]);
    preview(pitch);
    applyEdit(
      [
        {
          op: "add_notes",
          clip: currentClip.id,
          notes: [{ id, pos, dur, pitch, vel: 100 }],
        },
      ],
      `ノートを追加(${noteName(pitch)})`,
    );
  }

  function deleteNotes(ids: string[]) {
    const currentClip = clip;
    if (!currentClip || ids.length === 0) return;
    selected = new Set();
    applyEdit(
      [{ op: "remove_notes", clip: currentClip.id, ids }],
      `ノートを削除(${ids.length} 個)`,
    );
  }

  // クリップをまたいで使えるコピーバッファ(min pos からの相対位置で保持)
  let clipboard: { dpos: number; dur: number; pitch: number; vel: number }[] = [];

  function copySelection(cut: boolean) {
    const currentClip = clip;
    if (!currentClip) return;
    const notes = currentClip.notes.filter((n) => selected.has(n.id));
    if (notes.length === 0) return;
    const minPos = Math.min(...notes.map((n) => n.pos));
    clipboard = notes.map((n) => ({
      dpos: n.pos - minPos,
      dur: n.dur,
      pitch: n.pitch,
      vel: n.vel,
    }));
    if (cut) deleteNotes(notes.map((n) => n.id));
  }

  function paste() {
    const currentClip = clip;
    if (!currentClip || clipboard.length === 0) return;
    const anchor = Math.max(0, Math.min(hoverSnapTick, currentClip.length - 60));
    const notes = clipboard.map((c) => ({
      id: newNoteId(),
      pos: Math.max(0, Math.min(currentClip.length - 1, anchor + c.dpos)),
      dur: c.dur,
      pitch: c.pitch,
      vel: c.vel,
    }));
    selected = new Set(notes.map((n) => n.id));
    preview(notes[0].pitch);
    applyEdit(
      [{ op: "add_notes", clip: currentClip.id, notes }],
      `ノートを貼り付け(${notes.length} 個)`,
    );
  }

  /// キット図のパーツをクリック → 挿入カーソル位置に打ち込み + 行を表示
  function hitDrum(pitch: number) {
    const currentClip = clip;
    if (!currentClip) return;
    preview(pitch);
    drumHighlight = pitch;
    // 行が見えていなければスクロール
    if (scroller) {
      const y = (127 - pitch) * rowH;
      if (y < scroller.scrollTop + RULER_H || y > scroller.scrollTop + scroller.clientHeight - rowH * 2) {
        scroller.scrollTop = Math.max(0, y - scroller.clientHeight / 2);
      }
    }
    const pos = Math.max(0, Math.min(insertTick, currentClip.length - 60));
    const dur = Math.min(Math.max(snapTicks / 2, 120), currentClip.length - pos);
    const id = newNoteId();
    selected = new Set([id]);
    applyEdit(
      [{ op: "add_notes", clip: currentClip.id, notes: [{ id, pos, dur, pitch, vel: 100 }] }],
      `ドラムを打ち込み(${drumName(pitch)?.name ?? pitch})`,
    );
  }

  function onContextMenu(e: MouseEvent) {
    e.preventDefault();
    const hit = noteAt(e.offsetX, e.offsetY);
    if (!hit) return;
    deleteNotes(selected.has(hit.id) ? [...selected] : [hit.id]);
  }

  function onRulerClick(e: MouseEvent) {
    const currentClip = clip;
    if (!currentClip || !onSeek) return;
    onSeek(currentClip.start + e.offsetX / pxPerTick);
  }

  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!pianoRollStore.focus) return;
      const target = e.target as HTMLElement | null;
      if (target && (target.tagName === "TEXTAREA" || target.tagName === "INPUT")) return;
      if ((e.ctrlKey || e.metaKey) && e.code === "KeyC") {
        e.preventDefault();
        copySelection(false);
      } else if ((e.ctrlKey || e.metaKey) && e.code === "KeyX") {
        e.preventDefault();
        copySelection(true);
      } else if ((e.ctrlKey || e.metaKey) && e.code === "KeyV") {
        e.preventDefault();
        paste();
      } else if (e.code === "ArrowLeft" || e.code === "ArrowRight") {
        e.preventDefault();
        const currentClip = clip;
        if (!currentClip) return;
        const delta = e.code === "ArrowLeft" ? -snapTicks : snapTicks;
        insertTick = Math.max(
          0,
          Math.min(insertTick + delta, currentClip.length - 60),
        );
        // カーソルが画面端を超えたらスクロール追従(進行方向に余白を確保)
        if (scroller) {
          const ix = KEY_W + insertTick * pxPerTick;
          const view = scroller.clientWidth;
          const left = scroller.scrollLeft;
          const margin = 60;
          if (ix > left + view - margin) {
            scroller.scrollLeft = ix - view * 0.3;
          } else if (ix < left + KEY_W + margin) {
            scroller.scrollLeft = Math.max(0, ix - view * 0.7);
          }
        }
      } else if (e.key === "Delete" || e.key === "Backspace") {
        e.preventDefault();
        deleteNotes([...selected]);
      } else if (e.key === "Escape") {
        e.preventDefault();
        if (drag) {
          drag = null;
        } else if (selected.size > 0) {
          selected = new Set();
        } else {
          close();
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const bars = $derived(clip ? Math.ceil(clip.length / 3840) : 0);
</script>

{#if found}
  <div class="overlay">
    <div class="head">
      <div class="head-left">
        <span class="clip-name">{found.clip.name}</span>
        <span class="track-name">{found.track.name}</span>
        <code class="dim">{found.clip.id}</code>
      </div>
      <div class="head-right">
        {#if isDrum}
          <button
            class:kit-on={showKit}
            onclick={() => (showKit = !showKit)}
            title="ドラムキット図の表示/非表示"
          >
            🥁 キット
          </button>
        {/if}
        <label class="snap">
          スナップ
          <select bind:value={snapTicks}>
            {#each snapOptions as o (o.ticks)}
              <option value={o.ticks}>{o.label}</option>
            {/each}
          </select>
        </label>
        <span class="hint">クリック: 挿入カーソル(←/→ で移動) / ダブルクリック: 追加 / 右クリック・Del: 削除 / Ctrl+C/X/V: コピペ / Ctrl・Shift+ホイール: ズーム</span>
        <button onclick={close} title="閉じる(Esc)">✕</button>
      </div>
    </div>

    <!-- svelte-ignore a11y_no_static_element_interactions -->
    {#if isDrum && showKit}
      <DrumKit highlight={drumHighlight} onHit={hitDrum} />
    {/if}

    <div class="body" bind:this={scroller} onpointermove={updateHover}>
      <div class="grid" style="width:{KEY_W + contentW}px">
        <div class="corner" style="width:{KEY_W}px;height:{RULER_H}px"></div>
        <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
        <div
          class="ruler"
          style="width:{contentW}px;height:{RULER_H}px"
          onclick={onRulerClick}
        >
          {#each Array(bars) as _, i}
            <span class="bar-no" style="left:{i * 3840 * pxPerTick}px">{i + 1}</span>
          {/each}
        </div>
        <div class="keys" style="width:{KEY_W}px;height:{contentH}px">
          {#each Array(128) as _, row}
            {@const pitch = 127 - row}
            <div
              class="key"
              class:black={BLACK.has(pitch % 12)}
              class:drum-key={isDrum && drumName(pitch) !== undefined}
              style="height:{rowH}px"
            >
              {#if isDrum && drumName(pitch)}
                <span class="drum-label">{drumName(pitch)?.short}</span>
              {:else if pitch % 12 === 0}<span>{noteName(pitch)}</span>{/if}
            </div>
          {/each}
        </div>
        <div class="stack" style="width:{contentW}px;height:{contentH}px">
          <canvas bind:this={canvasEl} style="width:{contentW}px;height:{contentH}px"></canvas>
          <canvas
            class="overlay"
            bind:this={overlayEl}
            style="width:{contentW}px;height:{contentH}px"
            onpointerdown={onPointerDown}
            onpointermove={onPointerMove}
            onpointerup={onPointerUp}
            ondblclick={onDblClick}
            oncontextmenu={onContextMenu}
          ></canvas>
        </div>
      </div>
    </div>
  </div>
{/if}

<style>
  .overlay {
    position: absolute;
    inset: 0;
    z-index: 5;
    background: var(--bg);
    display: flex;
    flex-direction: column;
  }

  .head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 10px;
    padding: 6px 12px;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }

  .head-left {
    display: flex;
    align-items: baseline;
    gap: 10px;
    min-width: 0;
  }

  .clip-name {
    font-weight: 600;
    font-size: 14px;
  }

  .track-name {
    color: var(--text-dim);
    font-size: 12px;
  }

  .dim {
    color: var(--text-dim);
    opacity: 0.6;
    font-size: 10px;
  }

  .head-right {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .snap {
    font-size: 12px;
    color: var(--text-dim);
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .snap select {
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 2px 4px;
  }

  .hint {
    font-size: 11px;
    color: var(--text-dim);
  }

  .body {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }

  .grid {
    display: grid;
    grid-template-columns: auto 1fr;
    grid-template-rows: auto 1fr;
  }

  .corner {
    position: sticky;
    top: 0;
    left: 0;
    z-index: 3;
    background: var(--bg-panel);
    border-right: 1px solid var(--border);
    border-bottom: 1px solid var(--border);
  }

  .ruler {
    position: sticky;
    top: 0;
    z-index: 2;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
    cursor: pointer;
  }

  .bar-no {
    position: absolute;
    top: 0;
    font-size: 11px;
    color: var(--text-dim);
    padding-left: 4px;
    border-left: 1px solid var(--border);
    height: 100%;
    line-height: 22px;
    pointer-events: none;
  }

  .keys {
    position: sticky;
    left: 0;
    z-index: 2;
    background: var(--bg-panel);
    border-right: 1px solid var(--border);
  }

  .key {
    font-size: 9px;
    color: var(--text-dim);
    display: flex;
    align-items: center;
    justify-content: flex-end;
    padding-right: 4px;
    border-bottom: 1px solid rgba(255, 255, 255, 0.04);
  }

  .key.black {
    background: rgba(0, 0, 0, 0.35);
  }

  .key.drum-key {
    background: color-mix(in srgb, var(--accent) 10%, transparent);
  }

  .drum-label {
    color: var(--accent);
    font-weight: 700;
  }

  .kit-on {
    border-color: var(--accent-dim);
    color: var(--accent);
  }

  .stack {
    position: relative;
  }

  canvas {
    display: block;
    touch-action: none;
  }

  canvas.overlay {
    position: absolute;
    inset: 0;
  }
</style>
