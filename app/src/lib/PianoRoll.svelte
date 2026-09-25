<script lang="ts">
  import { onMount, tick as sveltick } from "svelte";
  import * as api from "./api";
  import { shouldYieldKey } from "./keys";
  import { buildBars } from "./barMap";
  import DrumKit from "./DrumKit.svelte";
  import Fretboard from "./Fretboard.svelte";
  import { drumName } from "./drumMap";
  import { newNoteId } from "./ids";
  import { noteClipboard, pianoRollStore } from "./selection.svelte";
  import type { Articulation, MidiClip, Note, Project, Track } from "./types";

  let {
    project,
    playheadTick = 0,
    playing = false,
    onSeek,
    pane = "main",
  }: {
    project: Project;
    playheadTick?: number;
    playing?: boolean;
    onSeek?: (tick: number) => void;
    /// "main" = 上ペイン(閉じると分割ごと閉じる)、"second" = 分割で開いた下ペイン
    pane?: "main" | "second";
  } = $props();

  /// このペインが表示しているクリップ
  const myFocus = $derived(pane === "main" ? pianoRollStore.focus : pianoRollStore.second);

  // ---- 対象クリップ(プロジェクト更新のたびに再導出 = AI の編集がライブ反映) ----

  const found = $derived.by((): { track: Track; clip: MidiClip } | null => {
    const focus = myFocus;
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
    if (myFocus && !found) {
      close();
    }
  });

  function close() {
    if (pane === "main") {
      pianoRollStore.focus = null;
      pianoRollStore.second = null;
      pianoRollStore.active = "main";
    } else {
      pianoRollStore.second = null;
      pianoRollStore.active = "main";
    }
  }

  // ---- 分割(2 ペイン): 別クリップを下に開いてコピペ・見比べ ----

  /// 分割ペインに開ける MIDI クリップ(自分以外)
  const otherClips = $derived.by(() => {
    const out: { track: Track; clip: MidiClip }[] = [];
    for (const t of project.tracks) {
      for (const c of t.clips) {
        if (c.kind === "midi" && c.id !== myFocus?.clipId) out.push({ track: t, clip: c });
      }
    }
    return out;
  });

  function openSplit(clipId: string) {
    const hit = otherClips.find((o) => o.clip.id === clipId);
    if (!hit) return;
    pianoRollStore.second = {
      clipId: hit.clip.id,
      clipName: hit.clip.name,
      trackId: hit.track.id,
      trackName: hit.track.name,
      anchorTick: 0,
    };
    pianoRollStore.active = "second";
  }

  /// 上下のクリップを入れ替える
  function swapPanes() {
    const a = pianoRollStore.focus;
    const b = pianoRollStore.second;
    if (!a || !b) return;
    pianoRollStore.focus = b;
    pianoRollStore.second = a;
  }

  // ---- 座標系 ----

  const KEY_W = 52;
  const RULER_H = 22;
  // ズーム(Ctrl+ホイール: 横、Shift+ホイール: 縦)
  let pxPerBeat = $state(60);
  let rowH = $state(14);
  const pxPerTick = $derived(pxPerBeat / 960);

  const clip = $derived(found?.clip ?? null);

  /// 編集範囲の長さ: ループクリップは繰り返す 1 回分、それ以外はクリップ長
  function lenOf(c: MidiClip): number {
    return c.loop && c.loop_len ? c.loop_len : c.length;
  }
  /// 再生ヘッドのクリップ内位置(ループ中は 1 回分の中に畳む)。クリップ外なら -1
  function relTick(c: MidiClip): number {
    const rel = playheadTick - c.start;
    if (rel < 0 || rel > c.length) return -1;
    return c.loop && c.loop_len ? rel % c.loop_len : rel;
  }

  const contentW = $derived(clip ? Math.max(lenOf(clip) * pxPerTick, 200) : 200);
  const contentH = $derived(128 * rowH);

  // 曲の絶対小節列(拍子イベント考慮)のうち、このクリップに重なる部分
  const songBars = $derived.by(() => {
    if (!clip) return [];
    const end = clip.start + lenOf(clip);
    return buildBars(project, end, 1, 0).filter(
      (b) => b.tick + b.len > clip.start && b.tick < end,
    );
  });

  const deviceRaw = $derived(found?.track.device as Record<string, unknown> | null | undefined);
  // ドラム: 内蔵 drum、または SoundFont の bank 128(GM ドラムキット)
  const isDrum = $derived(
    found?.track.device?.name === "drum" ||
      (deviceRaw?.type === "sf2" && deviceRaw?.bank === 128),
  );
  // フレット盤: 撥弦(pluck)と SoundFont(ドラムキット以外)のトラックで使える
  const isFrettable = $derived(
    found?.track.device?.name === "pluck" || (deviceRaw?.type === "sf2" && !isDrum),
  );
  // SoundFont のベース系プリセット(GM 32〜39)は既定でベース指板にする
  const defaultTuning = $derived.by((): "guitar" | "bass" => {
    const preset = deviceRaw?.preset;
    if (deviceRaw?.type === "sf2" && typeof preset === "number" && preset >= 32 && preset <= 39) {
      return "bass";
    }
    return "guitar";
  });
  let fretTuningOverride = $state<"guitar" | "bass" | null>(null);
  const fretTuning = $derived(fretTuningOverride ?? defaultTuning);

  // この楽器で効く奏法(glaux-dsp params.rs の articulations_for と同期を保つこと)
  const ARTS_BY_INSTRUMENT: Record<string, { art: Articulation; key: string; label: string }[]> = {
    subtractive: [
      { art: "palm_mute", key: "M", label: "ミュート" },
      { art: "staccato", key: "S", label: "スタッカート" },
      { art: "accent", key: "A", label: "アクセント" },
      { art: "vibrato", key: "V", label: "ビブラート" },
      { art: "bend", key: "B", label: "チョーキング" },
      { art: "legato", key: "T", label: "レガート" },
      { art: "portamento", key: "P", label: "ポルタメント" },
    ],
    drum: [{ art: "accent", key: "A", label: "アクセント" }],
    pluck: [
      { art: "palm_mute", key: "M", label: "ブリッジミュート" },
      { art: "staccato", key: "S", label: "スタッカート" },
      { art: "accent", key: "A", label: "アクセント" },
      { art: "vibrato", key: "V", label: "ビブラート" },
      { art: "bend", key: "B", label: "チョーキング" },
      { art: "legato", key: "T", label: "ハンマリング" },
      { art: "portamento", key: "P", label: "スライド" },
    ],
    sampler: [
      { art: "staccato", key: "S", label: "スタッカート" },
      { art: "accent", key: "A", label: "アクセント" },
      { art: "vibrato", key: "V", label: "ビブラート" },
      { art: "bend", key: "B", label: "チョーキング" },
      { art: "legato", key: "T", label: "レガート" },
      { art: "portamento", key: "P", label: "ポルタメント" },
    ],
    sf2: [
      { art: "staccato", key: "S", label: "スタッカート" },
      { art: "accent", key: "A", label: "アクセント" },
      { art: "vibrato", key: "V", label: "ビブラート" },
      { art: "bend", key: "B", label: "チョーキング" },
      { art: "legato", key: "T", label: "レガート" },
      { art: "portamento", key: "P", label: "ポルタメント" },
    ],
    fm: [
      { art: "palm_mute", key: "M", label: "ミュート" },
      { art: "staccato", key: "S", label: "スタッカート" },
      { art: "accent", key: "A", label: "アクセント" },
      { art: "vibrato", key: "V", label: "ビブラート" },
      { art: "bend", key: "B", label: "チョーキング" },
      { art: "legato", key: "T", label: "レガート" },
      { art: "portamento", key: "P", label: "ポルタメント" },
    ],
    wavetable: [
      { art: "palm_mute", key: "M", label: "ミュート" },
      { art: "staccato", key: "S", label: "スタッカート" },
      { art: "accent", key: "A", label: "アクセント" },
      { art: "vibrato", key: "V", label: "ビブラート" },
      { art: "bend", key: "B", label: "チョーキング" },
      { art: "legato", key: "T", label: "レガート" },
      { art: "portamento", key: "P", label: "ポルタメント" },
    ],
    // CLAP 音源: ビブラート・ベンドは音程の変化として送り、ミュート・アクセントは長さと強さで近づける
    clap: [
      { art: "palm_mute", key: "M", label: "ミュート(短く弱く)" },
      { art: "staccato", key: "S", label: "スタッカート" },
      { art: "accent", key: "A", label: "アクセント" },
      { art: "vibrato", key: "V", label: "ビブラート" },
      { art: "bend", key: "B", label: "チョーキング" },
      { art: "legato", key: "T", label: "レガート(重ねて送る)" },
      { art: "portamento", key: "P", label: "ポルタメント" },
    ],
  };
  const instrumentName = $derived(
    deviceRaw?.type === "sf2"
      ? isDrum
        ? "drum" // SF2 ドラムキットは奏法もドラム扱い(アクセントのみ)
        : "sf2"
      : deviceRaw?.type === "clap"
        ? "clap"
        : (found?.track.device?.name ?? "subtractive"),
  );
  const availableArts = $derived(
    ARTS_BY_INSTRUMENT[instrumentName] ?? ARTS_BY_INSTRUMENT.subtractive,
  );
  const artHint = $derived(
    availableArts.map((a) => `${a.key}=${a.label}`).join(" "),
  );
  /// 挿入カーソル(クリックで固定。キット/フレット打ち込み先。←/→ でスナップ移動)
  let insertTick = $state(0);
  let showKit = $state(true);
  let showFret = $state(true);
  let drumHighlight = $state<number | null>(null);

  let snapTicks = $state(480); // 1/8
  // T = 3 連符(PPQ 960: 1/4T=640, 1/8T=320, 1/16T=160)
  const snapOptions = [
    { label: "1 小節", ticks: 3840 },
    { label: "1/2", ticks: 1920 },
    { label: "1/4", ticks: 960 },
    { label: "1/4T", ticks: 640 },
    { label: "1/8", ticks: 480 },
    { label: "1/8T", ticks: 320 },
    { label: "1/16", ticks: 240 },
    { label: "1/16T", ticks: 160 },
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
    | { mode: "resize"; ids: string[]; startTick: number; dt: number }
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

  // ---- 描画する窓 ----
  // canvas はクリップ全体ではなく「見えている範囲 + 余白」の大きさにして、その位置に置く
  // (以前はクリップ全体の大きさで、64 小節で 15000×1750 が 2 枚・1 枚約 105MB。再生ヘッドが
  // 動くたびに全面を描き直していた)。描画はコンテンツ座標のまま、変換行列で窓の位置へずらす。
  // スクロールが窓の余白を越えたときだけ窓を動かして描き直す
  const WIN_MARGIN = 256;
  let win = $state({ x: 0, y: 0, w: 1, h: 1 });

  function updateWindow() {
    const el = scroller;
    if (!el) return;
    const viewW = Math.max(1, el.clientWidth - KEY_W);
    const viewH = Math.max(1, el.clientHeight - RULER_H);
    const w = Math.min(contentW, viewW + WIN_MARGIN * 2);
    const h = Math.min(contentH, viewH + WIN_MARGIN * 2);
    const visX = el.scrollLeft;
    const visY = el.scrollTop;
    const inside =
      win.w === w &&
      win.h === h &&
      visX >= win.x &&
      visX + viewW <= win.x + w &&
      visY >= win.y &&
      visY + viewH <= win.y + h;
    if (inside) return;
    win = {
      x: Math.max(0, Math.min(visX - WIN_MARGIN, contentW - w)),
      y: Math.max(0, Math.min(visY - WIN_MARGIN, contentH - h)),
      w,
      h,
    };
  }

  $effect(() => {
    const el = scroller;
    if (!el) return;
    let raf = 0;
    const schedule = () => {
      if (raf) return;
      raf = requestAnimationFrame(() => {
        raf = 0;
        updateWindow();
      });
    };
    el.addEventListener("scroll", schedule, { passive: true });
    const ro = new ResizeObserver(schedule);
    ro.observe(el);
    return () => {
      el.removeEventListener("scroll", schedule);
      ro.disconnect();
      if (raf) cancelAnimationFrame(raf);
    };
  });

  // ズーム・クリップの長さが変わったら窓を合わせ直す
  $effect(() => {
    void contentW;
    void contentH;
    updateWindow();
  });

  /** ノート層のポインタ位置(コンテンツ座標)。canvas は窓の位置に置いているので、その分を足す */
  const ex = (e: MouseEvent): number => e.offsetX + win.x;
  const ey = (e: MouseEvent): number => e.offsetY + win.y;

  function ensureSize(c: HTMLCanvasElement): CanvasRenderingContext2D {
    const dpr = window.devicePixelRatio || 1;
    const scale = Math.min(dpr, MAX_CANVAS_PX / win.w, MAX_CANVAS_PX / win.h);
    const w = Math.max(1, Math.round(win.w * scale));
    const h = Math.max(1, Math.round(win.h * scale));
    if (c.width !== w || c.height !== h) {
      c.width = w;
      c.height = h;
    }
    const g = c.getContext("2d")!;
    g.setTransform(scale, 0, 0, scale, -win.x * scale, -win.y * scale);
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
      g.fillStyle = BLACK.has(pitch % 12) ? "#1b1b1b" : "#232323";
      g.fillRect(0, y, contentW, rowH);
      if (pitch % 12 === 0) {
        // C の行の下線を強調
        g.fillStyle = "#3a3a3a";
        g.fillRect(0, y + rowH - 1, contentW, 1);
      }
    }

    // ドラムパーツ選択中の行をハイライト
    if (drumHighlight !== null) {
      g.fillStyle = "rgba(255, 194, 71, 0.08)";
      g.fillRect(0, (127 - drumHighlight) * rowH, contentW, rowH);
    }

    // 拍・小節線(拍子イベントを考慮した曲の絶対グリッド。クリップ相対に変換)
    const clipStart = currentClip.start;
    const clipEnd = clipStart + lenOf(currentClip);
    for (const bar of songBars) {
      if (bar.tick >= clipStart) {
        g.fillStyle = "#4a4a4a";
        g.fillRect((bar.tick - clipStart) * pxPerTick, 0, 1, contentH);
      }
      // 拍線(分母の音価 = 1 拍)
      const beatLen = (project.ppq * 4) / bar.den;
      g.fillStyle = "#333333";
      for (let t = bar.tick + beatLen; t < bar.tick + bar.len; t += beatLen) {
        if (t <= clipStart || t >= clipEnd) continue;
        g.fillRect((t - clipStart) * pxPerTick, 0, 1, contentH);
      }
    }
    // スナップグリッド(拍より細かいときだけ)
    if (snapTicks < 960) {
      g.fillStyle = "#2a2a2a";
      for (let t = 0; t <= lenOf(currentClip); t += snapTicks) {
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
      // ピッチカーブ(AI が描いたベンド等)。1 行 = 半音として折れ線で重ねる
      const curve = n.pitch_curve;
      if (curve && curve.length > 0 && w >= 6) {
        const cy = y + rowH / 2;
        g.beginPath();
        let first = true;
        const pts = [...curve];
        if (pts[0].tick > 0) pts.unshift({ tick: 0, cents: pts[0].cents });
        const last = pts[pts.length - 1];
        if (last.tick < n.dur) pts.push({ tick: n.dur, cents: last.cents });
        for (const p of pts) {
          const px = x + Math.min(p.tick, n.dur) * pxPerTick;
          const py = cy - (p.cents / 100) * rowH;
          if (first) {
            g.moveTo(px, py);
            first = false;
          } else {
            g.lineTo(px, py);
          }
        }
        g.strokeStyle = "rgba(255, 120, 200, 0.95)";
        g.lineWidth = 1.5;
        g.stroke();
      }
      // 奏法マーカー(M=ブリッジミュート / S=スタッカート / >=アクセント / ⌒=レガート / /=ポルタメント)
      const art = n.articulation;
      if (art && art !== "normal" && w >= 13 && rowH >= 9) {
        const label =
          art === "palm_mute"
            ? "M"
            : art === "staccato"
              ? "S"
              : art === "accent"
                ? ">"
                : art === "vibrato"
                  ? "~"
                  : art === "legato"
                    ? "⌒"
                    : art === "portamento"
                      ? "/"
                      : "↑";
        g.fillStyle = "rgba(12, 12, 12, 0.85)";
        g.font = `bold ${Math.min(rowH - 4, 10)}px sans-serif`;
        g.textBaseline = "middle";
        g.fillText(label, x + 3, y + rowH / 2 + 0.5);
      }
    }
  }

  function drawOverlay() {
    const c = overlayEl;
    const currentClip = clip;
    if (!c || !currentClip) return;
    const g = ensureSize(c);
    g.clearRect(0, 0, contentW, contentH);

    // 描画中のピッチカーブ(なぞった軌跡)
    if (curveDraw) {
      const n = currentClip.notes.find((m) => m.id === curveDraw!.noteId);
      if (n) {
        const cy = (127 - n.pitch) * rowH + rowH / 2;
        const pts = [...curveDraw.pts].sort((a, b) => a.t - b.t);
        g.strokeStyle = "rgba(255, 120, 200, 0.95)";
        g.lineWidth = 2;
        g.beginPath();
        pts.forEach((p, i) => {
          const px = (n.pos + p.t) * pxPerTick;
          const py = cy - (p.c / 100) * rowH;
          if (i === 0) g.moveTo(px, py);
          else g.lineTo(px, py);
        });
        g.stroke();
      }
    }

    // ドラッグ中の移動/リサイズゴースト
    if (drag?.mode === "move" && (drag.dt !== 0 || drag.dp !== 0)) {
      const ids = new Set(drag.ids);
      g.strokeStyle = "#ffd98a";
      g.fillStyle = "rgba(255, 194, 71, 0.35)";
      g.lineWidth = 1;
      for (const n of currentClip.notes) {
        if (!ids.has(n.id)) continue;
        const pos = Math.max(0, Math.min(lenOf(currentClip) - 1, n.pos + drag.dt));
        const pitch = Math.max(0, Math.min(127, n.pitch + drag.dp));
        g.beginPath();
        g.roundRect(pos * pxPerTick, (127 - pitch) * rowH + 1.5, Math.max(n.dur * pxPerTick, 4), rowH - 3, 3);
        g.fill();
        g.stroke();
      }
    }
    if (drag?.mode === "resize" && drag.dt !== 0) {
      const ids = new Set(drag.ids);
      g.strokeStyle = "#ffd98a";
      g.fillStyle = "rgba(255, 194, 71, 0.35)";
      for (const n of currentClip.notes) {
        if (!ids.has(n.id)) continue;
        const dur = Math.max(60, n.dur + drag.dt);
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
    if (insertTick <= lenOf(currentClip)) {
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
    if (hoverSnapTick <= lenOf(currentClip)) {
      const hx = hoverSnapTick * pxPerTick;
      g.fillStyle = "rgba(255, 194, 71, 0.28)";
      g.fillRect(hx, 0, 1, contentH);
    }

    // 再生ヘッド(クリップ内にあるときだけ)
    const rel = relTick(currentClip);
    if (rel >= 0) {
      g.fillStyle = "#ffc247";
      g.fillRect(rel * pxPerTick, 0, 1.5, contentH);
    }
  }

  // 静的層: 内容・選択・ズーム・スナップが変わったときだけ
  $effect(() => {
    void clip;
    void songBars;
    void selected;
    void snapTicks;
    void drumHighlight;
    void pxPerBeat;
    void rowH;
    void win;
    drawBase();
  });

  // 動的層: 高頻度更新(再生ヘッド・カーソル・ドラッグ)はこちらだけ再描画
  $effect(() => {
    void clip;
    void drag;
    void curveDraw;
    void playheadTick;
    void insertTick;
    void hoverSnapTick;
    void pxPerBeat;
    void rowH;
    void playing;
    void win;
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
    const rel = relTick(currentClip);
    if (rel < 0) return;
    const px = KEY_W + rel * pxPerTick;
    const view = scroller.clientWidth;
    const left = scroller.scrollLeft;
    if (px > left + view - 80 || px < left + KEY_W) {
      scroller.scrollLeft = Math.max(0, px - KEY_W - 80);
    }
  }

  // 開いたときにノートのある高さへスクロール
  $effect(() => {
    const focus = myFocus;
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
        Math.min(snapFloor(anchor), lenOf(currentClip) - 60),
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

  // ---- ピッチカーブの手描き(「〜 カーブ」モード) ----

  const MAX_CURVE_POINTS = 8;
  const MAX_CENTS = 2400;
  let curveMode = $state(false);
  /// 描いている途中の軌跡(ノート先頭からの tick, セント)
  let curveDraw = $state<{ noteId: string; pts: { t: number; c: number }[] } | null>(null);

  /// カーブを描く対象: 押した時刻にかかるノートのうち、選択中を優先し、なければ高さが近いもの
  function curveTarget(x: number, y: number): Note | null {
    const currentClip = clip;
    if (!currentClip) return null;
    const tick = x / pxPerTick;
    const covering = currentClip.notes.filter((n) => tick >= n.pos && tick <= n.pos + n.dur);
    if (covering.length === 0) return null;
    const sel = covering.filter((n) => selected.has(n.id));
    const pool = sel.length > 0 ? sel : covering;
    const rowPitch = 127 - y / rowH;
    return pool.reduce((a, b) => (Math.abs(a.pitch - rowPitch) <= Math.abs(b.pitch - rowPitch) ? a : b));
  }

  function curveSample(n: Note, x: number, y: number): { t: number; c: number } {
    const t = Math.max(0, Math.min(n.dur, x / pxPerTick - n.pos));
    const centerY = (127 - n.pitch) * rowH + rowH / 2;
    const c = Math.max(-MAX_CENTS, Math.min(MAX_CENTS, ((centerY - y) / rowH) * 100));
    return { t, c };
  }

  /// なぞった軌跡を最大 8 点に間引く(時間方向に等間隔で取り、線形補間で値を読む)
  function simplifyCurve(pts: { t: number; c: number }[]): { tick: number; cents: number }[] {
    const sorted = [...pts].sort((a, b) => a.t - b.t);
    if (sorted.length === 0) return [];
    const t0 = sorted[0].t;
    const t1 = sorted[sorted.length - 1].t;
    const valueAt = (t: number) => {
      let i = sorted.findIndex((p) => p.t >= t);
      if (i <= 0) return sorted[Math.max(0, i)].c;
      const a = sorted[i - 1];
      const b = sorted[i];
      const f = b.t === a.t ? 0 : (t - a.t) / (b.t - a.t);
      return a.c + (b.c - a.c) * f;
    };
    const n = t1 - t0 < 1 ? 1 : MAX_CURVE_POINTS;
    const out: { tick: number; cents: number }[] = [];
    for (let k = 0; k < n; k++) {
      const t = n === 1 ? t0 : t0 + ((t1 - t0) * k) / (n - 1);
      const tick = Math.round(t);
      if (out.length > 0 && out[out.length - 1].tick === tick) continue;
      out.push({ tick, cents: Math.round(valueAt(t)) });
    }
    return out;
  }

  function commitCurve(noteId: string, curve: { tick: number; cents: number }[]) {
    const currentClip = clip;
    if (!currentClip) return;
    applyEdit(
      [{ op: "update_notes", clip: currentClip.id, changes: [{ id: noteId, pitch_curve: curve }] }],
      curve.length === 0 ? "ピッチカーブを削除" : "ピッチカーブを描画",
    );
  }

  function nearRightEdge(n: Note, x: number): boolean {
    const right = (n.pos + n.dur) * pxPerTick;
    return right - x < 6 && right - x > -3;
  }

  function preview(pitch: number) {
    const trackId = found?.track.id;
    if (trackId) api.previewNote(trackId, pitch).catch(() => {});
  }

  // ---- ベロシティの帯(下端に固定、横スクロールはノートと連動) ----

  const VEL_H = 64;
  const VEL_PAD = 4;
  let showVel = $state(true);
  let velEl: HTMLCanvasElement | undefined = $state();
  /// ドラッグ中の変更(ノート ID → 元の強さ)と、掴んだノートの増減量
  let velDrag = $state<{ base: Map<string, number>; anchor: string; delta: number } | null>(null);

  function velFromY(y: number): number {
    const t = 1 - (y - VEL_PAD) / (VEL_H - VEL_PAD * 2);
    return Math.max(1, Math.min(127, Math.round(t * 127)));
  }

  function shownVel(n: Note): number {
    const d = velDrag;
    const base = d?.base.get(n.id);
    if (d && base !== undefined) return Math.max(1, Math.min(127, base + d.delta));
    return n.vel;
  }

  function velBarRect(n: Note): { x: number; w: number } {
    return { x: n.pos * pxPerTick, w: Math.max(3, Math.min(8, n.dur * pxPerTick - 1)) };
  }

  function drawVel() {
    const c = velEl;
    const currentClip = clip;
    if (!c || !currentClip || !showVel) return;
    const dpr = window.devicePixelRatio || 1;
    const scale = Math.min(dpr, MAX_CANVAS_PX / win.w);
    const w = Math.max(1, Math.round(win.w * scale));
    const h = Math.round(VEL_H * scale);
    if (c.width !== w || c.height !== h) {
      c.width = w;
      c.height = h;
    }
    const g = c.getContext("2d")!;
    g.setTransform(scale, 0, 0, scale, -win.x * scale, 0);
    g.clearRect(0, 0, contentW, VEL_H);
    g.fillStyle = "#1b1b1b";
    g.fillRect(0, 0, contentW, VEL_H);
    // 目安線(25 / 50 / 75 / 100%)
    g.fillStyle = "#2a2a2a";
    for (const f of [0.25, 0.5, 0.75, 1]) {
      g.fillRect(0, Math.round(VEL_PAD + (1 - f) * (VEL_H - VEL_PAD * 2)), contentW, 1);
    }
    // 小節線
    g.fillStyle = "#333333";
    for (const b of songBars) {
      g.fillRect((b.tick - currentClip.start) * pxPerTick, 0, 1, VEL_H);
    }
    for (const n of currentClip.notes) {
      const v = shownVel(n);
      const { x, w } = velBarRect(n);
      const bh = (v / 127) * (VEL_H - VEL_PAD * 2);
      const isSel = selected.has(n.id);
      g.fillStyle = isSel ? "rgba(255, 194, 71, 0.95)" : "rgba(94, 156, 224, 0.9)";
      g.fillRect(x, VEL_H - VEL_PAD - bh, w, bh);
      // 頭に丸(掴む場所の目印)
      g.beginPath();
      g.arc(x + w / 2, VEL_H - VEL_PAD - bh, 2.5, 0, Math.PI * 2);
      g.fill();
    }
  }

  $effect(() => {
    void clip;
    void selected;
    void songBars;
    void pxPerBeat;
    void velDrag;
    void showVel;
    void win;
    drawVel();
  });

  /// x 位置の縦棒のノート(複数重なるときは開始位置が近い方)
  function velNoteAt(x: number): Note | null {
    const currentClip = clip;
    if (!currentClip) return null;
    let best: Note | null = null;
    let bestDist = Infinity;
    for (const n of currentClip.notes) {
      const { x: bx, w } = velBarRect(n);
      if (x < bx - 3 || x > bx + w + 3) continue;
      const d = Math.abs(x - (bx + w / 2));
      if (d < bestDist) {
        bestDist = d;
        best = n;
      }
    }
    return best;
  }

  function onVelDown(e: PointerEvent) {
    if (e.button !== 0) return;
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const n = velNoteAt(e.clientX - rect.left + win.x);
    if (!n) return;
    const currentClip = clip;
    if (!currentClip) return;
    // 選択中のノートを掴んだら選択中すべてを同じ量だけ動かす
    let ids: string[];
    if (selected.has(n.id) && selected.size > 1) {
      ids = [...selected];
    } else {
      ids = [n.id];
      selected = new Set([n.id]);
    }
    const base = new Map<string, number>();
    for (const m of currentClip.notes) if (ids.includes(m.id)) base.set(m.id, m.vel);
    velDrag = { base, anchor: n.id, delta: velFromY(e.clientY - rect.top) - n.vel };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }

  function onVelMove(e: PointerEvent) {
    const d = velDrag;
    if (!d) return;
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const anchorBase = d.base.get(d.anchor) ?? 64;
    velDrag = { ...d, delta: velFromY(e.clientY - rect.top) - anchorBase };
  }

  function onVelUp() {
    const d = velDrag;
    const currentClip = clip;
    if (!d || !currentClip) {
      velDrag = null;
      return;
    }
    const changes = [...d.base.entries()]
      .map(([id, v]) => ({ id, vel: Math.max(1, Math.min(127, v + d.delta)) }))
      .filter((c) => c.vel !== d.base.get(c.id));
    if (changes.length > 0) {
      const one = changes.length === 1 ? `(${changes[0].vel})` : `(${changes.length} 個)`;
      // 反映されるまでのちらつきを避けるため、ドラッグ表示は編集の完了後に消す
      applyEdit(
        [{ op: "update_notes", clip: currentClip.id, changes }],
        `ベロシティを変更${one}`,
      ).finally(() => (velDrag = null));
    } else {
      velDrag = null;
    }
  }

  async function applyEdit(commands: unknown[], label: string) {
    try {
      await api.applyEdit(commands, label);
    } catch (e) {
      console.error(e);
    }
  }

  // ---- ポルタメントの滑る時間(選択中のポルタメントのノートだけ) ----
  const GLIDE_CHOICES = [40, 80, 150, 250, 400, 800];
  const selectedPorta = $derived(
    (clip?.notes ?? []).filter((n) => selected.has(n.id) && n.articulation === "portamento"),
  );
  // 選んだ音がみな同じなら、その値(個別指定なしは "0" = トラックの設定)。ばらばらなら ""
  const glideValue = $derived.by(() => {
    const vals = new Set(selectedPorta.map((n) => String(n.glide_ms ?? 0)));
    return vals.size === 1 ? [...vals][0] : "";
  });
  function setNoteGlide(value: string) {
    const currentClip = clip;
    if (!currentClip || !value || selectedPorta.length === 0) return;
    const ms = Number(value);
    applyEdit(
      [
        {
          op: "update_notes",
          clip: currentClip.id,
          changes: selectedPorta.map((n) => ({ id: n.id, glide_ms: ms })),
        },
      ],
      ms > 0
        ? `ポルタメントの滑る時間を ${ms}ms に(${selectedPorta.length} ノート)`
        : `ポルタメントの滑る時間をトラックの設定に戻す(${selectedPorta.length} ノート)`,
    );
  }

  // ---- スウィング(選択中のノート、無ければクリップ全体) ----
  let swingGrid = $state(480);
  let swingMsg = $state<string | null>(null);
  async function applySwing(value: string) {
    if (!clip || !value) return;
    const swing = Number(value);
    const ids = selected.size > 0 ? [...selected] : null;
    try {
      const r = await api.swingClip(clip.id, ids, swingGrid, swing);
      swingMsg = r.changed > 0 ? `${r.changed} ノートを動かしました(Ctrl+Z で戻せます)` : "動かすノート(裏拍の音)がありません";
    } catch (e) {
      swingMsg = String(e);
    }
    setTimeout(() => (swingMsg = null), 3000);
  }

  function onPointerDown(e: PointerEvent) {
    if (e.button === 2) return; // 右クリックは contextmenu で処理
    const currentClip = clip;
    if (!currentClip) return;
    const x = ex(e);
    const y = ey(e);
    if (curveMode) {
      const n = curveTarget(x, y);
      if (!n) return;
      (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
      selected = new Set([n.id]);
      curveDraw = { noteId: n.id, pts: [curveSample(n, x, y)] };
      return;
    }
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
        // 選択中のノートの端なら、選択全体をまとめてリサイズ
        drag = {
          mode: "resize",
          ids: [...(selected.has(hit.id) ? selected : new Set([hit.id]))],
          startTick: x / pxPerTick,
          dt: 0,
        };
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
    hoverTick = Math.max(0, (e.clientX - rect.left + win.x) / pxPerTick);
  }

  function onPointerMove(e: PointerEvent) {
    if (curveDraw) {
      const n = clip?.notes.find((m) => m.id === curveDraw!.noteId);
      if (n) curveDraw = { ...curveDraw, pts: [...curveDraw.pts, curveSample(n, ex(e), ey(e))] };
      return;
    }
    updateHover(e);
    if (!drag) {
      // カーソル形状
      const hit = noteAt(ex(e), ey(e));
      const el = e.currentTarget as HTMLElement;
      el.style.cursor = hit ? (nearRightEdge(hit, ex(e)) ? "ew-resize" : "move") : "default";
      return;
    }
    const x = ex(e);
    const y = ey(e);
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
    if (curveDraw) {
      const d = curveDraw;
      curveDraw = null;
      commitCurve(d.noteId, simplifyCurve(d.pts));
      return;
    }
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
          Math.min(snapFloor(d.x0 / pxPerTick), lenOf(currentClip) - 60),
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
          pos: Math.max(0, Math.min(lenOf(currentClip) - 1, n.pos + d.dt)),
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
      const changes = currentClip.notes
        .filter((n) => d.ids.includes(n.id))
        .map((n) => ({
          id: n.id,
          dur: Math.max(60, Math.min(lenOf(currentClip) - n.pos, n.dur + d.dt)),
        }));
      if (changes.length > 0) {
        applyEdit(
          [{ op: "update_notes", clip: currentClip.id, changes }],
          changes.length === 1
            ? "ノートの長さを変更"
            : `ノートの長さを変更(${changes.length} 個)`,
        );
      }
    }
  }

  function onDblClick(e: MouseEvent) {
    if (curveMode) return;
    const currentClip = clip;
    if (!currentClip) return;
    if (noteAt(ex(e), ey(e))) return;
    const pos = Math.max(0, Math.min(snapFloor(ex(e) / pxPerTick), lenOf(currentClip) - 60));
    const pitch = Math.max(0, Math.min(127, 127 - Math.floor(ey(e) / rowH)));
    const dur = Math.min(snapTicks, lenOf(currentClip) - pos);
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
  function copySelection(cut: boolean) {
    const currentClip = clip;
    if (!currentClip) return;
    const notes = currentClip.notes.filter((n) => selected.has(n.id));
    if (notes.length === 0) return;
    const minPos = Math.min(...notes.map((n) => n.pos));
    // クリップボードは全ペイン・全クリップで共有(分割ペイン間のコピペ)
    noteClipboard.items = notes.map((n) => ({
      dpos: n.pos - minPos,
      dur: n.dur,
      pitch: n.pitch,
      vel: n.vel,
      articulation: n.articulation,
    }));
    if (cut) deleteNotes(notes.map((n) => n.id));
  }

  function paste() {
    const currentClip = clip;
    const clipboard = noteClipboard.items;
    if (!currentClip || clipboard.length === 0) return;
    const anchor = Math.max(0, Math.min(hoverSnapTick, lenOf(currentClip) - 60));
    const notes = clipboard.map((c) => ({
      id: newNoteId(),
      pos: Math.max(0, Math.min(lenOf(currentClip) - 1, anchor + c.dpos)),
      dur: c.dur,
      pitch: c.pitch,
      vel: c.vel,
      ...(c.articulation && c.articulation !== "normal" ? { articulation: c.articulation } : {}),
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
    const pos = Math.max(0, Math.min(insertTick, lenOf(currentClip) - 60));
    const dur = Math.min(Math.max(snapTicks / 2, 120), lenOf(currentClip) - pos);
    const id = newNoteId();
    selected = new Set([id]);
    applyEdit(
      [{ op: "add_notes", clip: currentClip.id, notes: [{ id, pos, dur, pitch, vel: 100 }] }],
      `ドラムを打ち込み(${drumName(pitch)?.name ?? pitch})`,
    );
  }

  /// フレット盤のクリック → 挿入カーソル位置に打ち込み(スナップ長)
  function hitFret(pitch: number) {
    const currentClip = clip;
    if (!currentClip) return;
    preview(pitch);
    drumHighlight = pitch;
    if (scroller) {
      const y = (127 - pitch) * rowH;
      if (y < scroller.scrollTop + RULER_H || y > scroller.scrollTop + scroller.clientHeight - rowH * 2) {
        scroller.scrollTop = Math.max(0, y - scroller.clientHeight / 2);
      }
    }
    const pos = Math.max(0, Math.min(insertTick, lenOf(currentClip) - 60));
    const dur = Math.min(snapTicks, lenOf(currentClip) - pos);
    const id = newNoteId();
    selected = new Set([id]);
    applyEdit(
      [{ op: "add_notes", clip: currentClip.id, notes: [{ id, pos, dur, pitch, vel: 100 }] }],
      `ノートを打ち込み(${noteName(pitch)})`,
    );
  }

  function onContextMenu(e: MouseEvent) {
    if (curveMode) {
      // カーブモードの右クリック: そのノートのカーブを消す
      e.preventDefault();
      const n = curveTarget(ex(e), ey(e));
      if (n && n.pitch_curve && n.pitch_curve.length > 0) commitCurve(n.id, []);
      return;
    }
    e.preventDefault();
    const hit = noteAt(ex(e), ey(e));
    if (!hit) return;
    deleteNotes(selected.has(hit.id) ? [...selected] : [hit.id]);
  }

  const ART_LABELS: Record<Articulation, string> = {
    normal: "通常",
    palm_mute: "ブリッジミュート",
    staccato: "スタッカート",
    accent: "アクセント",
    vibrato: "ビブラート",
    bend: "チョーキング",
    legato: "レガート",
    portamento: "ポルタメント",
  };

  /// 選択ノートの奏法をトグルする(全部が同じ奏法なら通常に戻す)
  function toggleArticulation(art: Articulation) {
    const currentClip = clip;
    if (!currentClip || selected.size === 0) return;
    const notes = currentClip.notes.filter((n) => selected.has(n.id));
    if (notes.length === 0) return;
    const allHave = notes.every((n) => (n.articulation ?? "normal") === art);
    const target: Articulation = allHave ? "normal" : art;
    applyEdit(
      [
        {
          op: "update_notes",
          clip: currentClip.id,
          changes: notes.map((n) => ({ id: n.id, articulation: target })),
        },
      ],
      allHave
        ? `${ART_LABELS[art]}を解除(${notes.length} ノート)`
        : `${ART_LABELS[art]}を設定(${notes.length} ノート)`,
    );
  }

  function onRulerClick(e: MouseEvent) {
    const currentClip = clip;
    if (!currentClip || !onSeek) return;
    onSeek(currentClip.start + e.offsetX / pxPerTick);
  }

  // ルーラーに出す小節(小節頭がクリップ内にあるもの。番号は曲の絶対小節)
  const rulerBars = $derived(
    clip ? songBars.filter((b) => b.tick >= clip.start) : [],
  );

  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      // 分割時はアクティブなペイン(最後にクリックした方)だけがキーを受ける
      if (!myFocus || pianoRollStore.active !== pane) return;
      if (shouldYieldKey(e)) return;
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
          Math.min(insertTick + delta, lenOf(currentClip) - 60),
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
      } else if (
        !e.ctrlKey &&
        !e.metaKey &&
        !e.altKey &&
        (e.code === "KeyM" ||
          e.code === "KeyS" ||
          e.code === "KeyA" ||
          e.code === "KeyV" ||
          e.code === "KeyB" ||
          e.code === "KeyT" ||
          e.code === "KeyP")
      ) {
        // 奏法トグル(選択ノートに対して。この楽器で効くものだけ)
        if (selected.size === 0) return;
        const key = e.code.slice(3); // "KeyM" → "M"
        const entry = availableArts.find((a) => a.key === key);
        if (!entry) return;
        e.preventDefault();
        toggleArticulation(entry.art);
      } else if (e.key === "Delete" || e.key === "Backspace") {
        e.preventDefault();
        deleteNotes([...selected]);
      } else if (e.key === "Escape") {
        e.preventDefault();
        if (curveDraw) {
          curveDraw = null;
        } else if (curveMode) {
          curveMode = false;
        } else if (drag) {
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
</script>

{#if found}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="overlay"
    class:inactive={pianoRollStore.second !== null && pianoRollStore.active !== pane}
    onpointerdowncapture={() => (pianoRollStore.active = pane)}
  >
    <div class="head">
      <div class="head-left">
        {#if pianoRollStore.second}
          <span class="pane-tag">{pane === "main" ? "上" : "下"}</span>
        {/if}
        <span class="clip-name">{found.clip.name}</span>
        <span class="track-name">{found.track.name}</span>
        {#if found.clip.loop && found.clip.loop_len}
          <span
            class="loop-tag"
            title="ループクリップ: ここで編集した範囲がクリップの長さまで繰り返し鳴ります"
          >🔁 {(found.clip.loop_len / (project.ppq * 4)).toFixed(found.clip.loop_len % (project.ppq * 4) === 0 ? 0 : 2)} 小節ぶんを繰り返し</span
          >
        {/if}
        <code class="dim">{found.clip.id}</code>
      </div>
      <div class="head-right">
        {#if pane === "main"}
          <select
            class="split"
            value=""
            onchange={(e) => {
              const v = (e.currentTarget as HTMLSelectElement).value;
              if (v) openSplit(v);
              (e.currentTarget as HTMLSelectElement).value = "";
            }}
            title="別のクリップを下に開いて見比べ・コピペ(Ctrl+C → 下をクリック → Ctrl+V)"
          >
            <option value="">⫶ 分割…</option>
            {#each otherClips as o (o.clip.id)}
              <option value={o.clip.id}>{o.track.name} / {o.clip.name}</option>
            {/each}
          </select>
        {:else}
          <button onclick={swapPanes} title="上下のクリップを入れ替える">⇅</button>
        {/if}
        {#if isDrum}
          <button
            class:kit-on={showKit}
            onclick={() => (showKit = !showKit)}
            title="ドラムキット図の表示/非表示"
          >
            🥁 キット
          </button>
        {/if}
        {#if isFrettable}
          <select
            class="tuning"
            value={fretTuning}
            onchange={(e) =>
              (fretTuningOverride = (e.currentTarget as HTMLSelectElement).value as
                | "guitar"
                | "bass")}
            title="フレット盤のチューニング"
          >
            <option value="guitar">🎸 ギター(6 弦)</option>
            <option value="bass">🎸 ベース(4 弦)</option>
          </select>
          <button
            class:kit-on={showFret}
            onclick={() => (showFret = !showFret)}
            title="フレット盤の表示/非表示"
          >
            フレット
          </button>
        {/if}
        <button
          class:kit-on={curveMode}
          onclick={() => (curveMode = !curveMode)}
          title="ピッチカーブを手で描く: ノートの上をなぞると、その高さのずれ(1 行 = 半音)がカーブになる。右クリックでカーブを消す"
        >
          〜 カーブ
        </button>
        <button
          class:kit-on={showVel}
          onclick={() => (showVel = !showVel)}
          title="ベロシティ(音の強さ)の帯の表示/非表示。縦棒を上下にドラッグで変更、選択中のノートはまとめて変わる"
        >
          ベロシティ
        </button>
        <label class="snap" title="裏拍の音をハネさせる(選択中のノート、無ければクリップ全体)。表の音と長さは変えない。同じ設定なら何度掛けても同じ">
          スウィング
          <select bind:value={swingGrid}>
            <option value={480}>8 分</option>
            <option value={240}>16 分</option>
          </select>
          <select
            value=""
            onchange={(e) => {
              const el = e.currentTarget as HTMLSelectElement;
              applySwing(el.value);
              el.value = "";
            }}
          >
            <option value="">掛ける…</option>
            <option value="0.5">ストレート(50%)</option>
            <option value="0.58">軽め(58%)</option>
            <option value="0.62">中くらい(62%)</option>
            <option value="0.6667">3 連シャッフル(67%)</option>
            <option value="0.75">付点(75%)</option>
          </select>
        </label>
        {#if swingMsg}<span class="swing-msg">{swingMsg}</span>{/if}
        {#if selectedPorta.length > 0}
          <label class="snap" title="選んだポルタメント(P)のノートが直前の音から滑る時間。トラック全体の既定は音作りビューの「つなぎ」で">
            滑る時間
            <select value={glideValue} onchange={(e) => setNoteGlide((e.currentTarget as HTMLSelectElement).value)}>
              {#if glideValue === ""}<option value="">(ばらばら)</option>{/if}
              <option value="0">トラックの設定</option>
              {#each GLIDE_CHOICES as ms (ms)}
                <option value={String(ms)}>{ms}ms</option>
              {/each}
              {#if glideValue !== "" && glideValue !== "0" && !GLIDE_CHOICES.includes(Number(glideValue))}
                <option value={glideValue}>{glideValue}ms</option>
              {/if}
            </select>
          </label>
        {/if}
        <label class="snap">
          スナップ
          <select bind:value={snapTicks}>
            {#each snapOptions as o (o.ticks)}
              <option value={o.ticks}>{o.label}</option>
            {/each}
          </select>
        </label>
        <span
          class="hint"
          title={`ドラッグ: 複数選択(まとめて移動・端で長さ変更)\nCtrl+C/X/V: コピペ(別クリップも可)\n奏法: ${artHint}\nダブルクリック: 追加 / 右クリック・Del: 削除\nCtrl・Shift+ホイール: ズーム\nベロシティ: 下の帯の縦棒を上下にドラッグ`}
          >ドラッグ: 複数選択(まとめて移動・端で長さ変更) / Ctrl+C/X/V: コピペ(別クリップも可) / 奏法: {artHint} / ダブルクリック: 追加 / 右クリック・Del: 削除 / Ctrl・Shift+ホイール: ズーム</span
        >
        <button onclick={close} title={pane === "main" ? "閉じる(Esc)" : "この分割ペインを閉じる(Esc)"}>✕</button>
      </div>
    </div>

    <!-- svelte-ignore a11y_no_static_element_interactions -->
    {#if isDrum && showKit}
      <DrumKit highlight={drumHighlight} onHit={hitDrum} />
    {/if}
    {#if isFrettable && showFret}
      <Fretboard highlight={drumHighlight} onHit={hitFret} tuning={fretTuning} />
    {/if}

    <div class="body" bind:this={scroller} onpointermove={updateHover}>
      <!-- Flex 行構成: grid アイテムの sticky は自分のグリッド領域内でしか
           動けず無効化されるため、行(上固定)+ 列(左固定)で組む -->
      <div class="grid" style="width:{KEY_W + contentW}px">
        <div class="top-row" style="height:{RULER_H}px">
          <div class="corner" style="width:{KEY_W}px"></div>
          <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
          <div class="ruler" style="width:{contentW}px" onclick={onRulerClick}>
            {#each rulerBars as bar (bar.index)}
              <span class="bar-no" style="left:{(bar.tick - (clip?.start ?? 0)) * pxPerTick}px">
                {bar.index + 1}{#if bar.sigChange}<span class="sig-chip">{bar.num}/{bar.den}</span>{/if}
              </span>
            {/each}
          </div>
        </div>
        <div class="content-row">
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
            <canvas
              class="win-layer"
              bind:this={canvasEl}
              style="left:{win.x}px;top:{win.y}px;width:{win.w}px;height:{win.h}px"
            ></canvas>
            <!-- クラス名を .overlay(パネルのルート)と絶対に被せないこと:
                 被るとルート用の z-index:5 + 不透明背景がこの canvas に当たり、
                 グリッド・ノート・鍵盤が全部この canvas の下に隠れる(過去の実バグ) -->
            <canvas
              class="note-layer"
              class:curve-mode={curveMode}
              bind:this={overlayEl}
              style="left:{win.x}px;top:{win.y}px;width:{win.w}px;height:{win.h}px"
              onpointerdown={onPointerDown}
              onpointermove={onPointerMove}
              onpointerup={onPointerUp}
              ondblclick={onDblClick}
              oncontextmenu={onContextMenu}
            ></canvas>
          </div>
        </div>
        {#if showVel}
          <div class="vel-row" style="height:{VEL_H}px">
            <div class="vel-corner" style="width:{KEY_W}px" title="ベロシティ(音の強さ 1〜127)">Vel</div>
            <div class="vel-track" style="width:{contentW}px;height:{VEL_H}px">
            <canvas
              class="vel-layer"
              bind:this={velEl}
              style="left:{win.x}px;width:{win.w}px;height:{VEL_H}px"
              title={velDrag
                ? `ベロシティ ${Math.max(1, Math.min(127, (velDrag.base.get(velDrag.anchor) ?? 0) + velDrag.delta))}`
                : "縦棒を上下にドラッグで音の強さを変更(選択中のノートはまとめて変わる)"}
              onpointerdown={onVelDown}
              onpointermove={onVelMove}
              onpointerup={onVelUp}
              onpointercancel={() => (velDrag = null)}
            ></canvas>
            </div>
          </div>
        {/if}
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

  /* 分割時: 非アクティブなペインのヘッダを少し落として、キーがどちらに効くか示す */
  .overlay.inactive .head {
    opacity: 0.6;
  }

  .pane-tag {
    font-size: 10px;
    padding: 1px 6px;
    border-radius: 3px;
    background: color-mix(in srgb, var(--accent) 25%, transparent);
    color: var(--accent);
  }

  .split {
    font-size: 11px;
    max-width: 160px;
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
    white-space: nowrap;
    flex-shrink: 0;
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

  /* ヘッダーは 1 行に保つ(ボタン類は折り返さず、説明文だけ省略表示) */
  .loop-tag {
    font-size: 11px;
    color: var(--accent);
  }

  .head-right {
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
    white-space: nowrap;
  }

  .swing-msg {
    font-size: 11px;
    color: var(--accent);
    white-space: nowrap;
  }

  .head-right > button,
  .head-right > select,
  .head-right > .snap {
    flex-shrink: 0;
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
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .body {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }

  .top-row {
    display: flex;
    position: sticky;
    top: 0;
    z-index: 3;
  }

  .content-row {
    display: flex;
  }

  .corner {
    position: sticky;
    left: 0;
    z-index: 4;
    flex-shrink: 0;
    background: var(--bg-panel);
    border-right: 1px solid var(--border);
    border-bottom: 1px solid var(--border);
  }

  .ruler {
    position: relative;
    flex-shrink: 0;
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
    white-space: nowrap;
  }

  .sig-chip {
    margin-left: 3px;
    padding: 0 3px;
    border-radius: 3px;
    font-size: 9px;
    background: color-mix(in srgb, var(--accent) 25%, transparent);
    color: var(--accent);
  }

  .keys {
    position: sticky;
    left: 0;
    z-index: 2;
    flex-shrink: 0;
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

  .tuning {
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 2px 4px;
    font-size: 11px;
  }

  .kit-on {
    border-color: var(--accent-dim);
    color: var(--accent);
  }

  .stack {
    position: relative;
    flex-shrink: 0;
  }

  /* ベロシティの帯: 縦スクロールしても下端に残る */
  .vel-row {
    display: flex;
    position: sticky;
    bottom: 0;
    z-index: 3;
    border-top: 1px solid var(--border);
  }

  .vel-corner {
    position: sticky;
    left: 0;
    z-index: 4;
    flex-shrink: 0;
    background: var(--bg-panel);
    border-right: 1px solid var(--border);
    font-size: 10px;
    color: var(--text-dim);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .vel-layer {
    cursor: ns-resize;
  }

  .note-layer.curve-mode {
    cursor: crosshair;
  }

  canvas {
    display: block;
    touch-action: none;
  }

  /* canvas は窓(見えている範囲 + 余白)の大きさで、その位置に置く */
  canvas.note-layer,
  canvas.win-layer,
  canvas.vel-layer {
    position: absolute;
  }

  .vel-track {
    position: relative;
    flex-shrink: 0;
  }
</style>
