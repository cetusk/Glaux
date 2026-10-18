<script lang="ts">
  // 曲から起こす作業場。参考曲(参考の印の音声トラック)から、格子(テンポ・小節頭・拍子)→ パート → 確かめる、の順に進む。
  // 1. 格子: 参考曲の波形に推定した拍・小節線・和音を重ね、ずれていれば小節頭・拍の取り方・拍子を直して曲に使う
  // 2. パート: 起こすパート(歌・ベース・ピアノ・ギター・その他)と音の素(参考曲そのまま / 分けた音のファイル)を選んで起こす
  // 3. 確かめる: 重ねて聴く・遅く聴く・タイムラインで直す。小節ごとの和音(コード譜)も見られる
  import { onMount, untrack } from "svelte";
  import { open as pickFile } from "@tauri-apps/plugin-dialog";
  import Icon from "./Icon.svelte";
  import * as api from "./api";
  import { color } from "./palette";
  import { plural, tr } from "./i18n.svelte";
  import { showError, showToast } from "./toast.svelte";
  import { importReferenceFile, referenceClipId, referenceStore, referenceTrack, setReferenceMode, setSpeed } from "./reference.svelte";
  import { viewStore } from "./selection.svelte";
  import { secondsToTick, tickToSeconds } from "./barMap";
  import type { Project } from "./types";

  let {
    project,
    projectVersion,
    playheadTick,
    playing,
  }: { project: Project; projectVersion: number; playheadTick: number; playing: boolean } = $props();

  const refTrack = $derived(referenceTrack(project));
  const clipId = $derived(referenceClipId(project));

  // ---- 手順と起こし方 ----
  let step = $state<1 | 2 | 3>(1);
  /** new = 白紙から(曲のテンポ・拍子を参考曲に合わせて起こす)、import = 今の曲に一部を取り込む(今の曲のテンポのまま) */
  const songHasNotes = (p: Project) =>
    p.tracks.some((t) => !t.reference && t.clips.some((c) => c.kind === "midi" && c.notes.length > 0));
  // 開いたときの曲で決める(音のある曲なら取り込む)
  let mode = $state<"new" | "import">(untrack(() => (songHasNotes(project) ? "import" : "new")));

  // ---- 1. 格子 ----
  let opts = $state<api.GridOptions>({ downbeat_shift: 0, tempo_factor: 1, beats_per_bar: null, no_pickup: null });
  let ws = $state<api.ReconWorkspace | null>(null);
  let loading = $state(false);
  let loadErr = $state("");
  let withSections = $state(true);
  let applying = $state(false);
  let peaks = $state<[number, number][]>([]);

  async function load() {
    if (!clipId) return;
    loading = true;
    loadErr = "";
    try {
      ws = await api.reconWorkspace(clipId, $state.snapshot(opts));
    } catch (e) {
      loadErr = String(e);
      ws = null;
    } finally {
      loading = false;
    }
  }

  // 参考曲が替わったら読み直す(格子の直しも戻す)
  let loadedClip = "";
  $effect(() => {
    const id = clipId;
    if (!id || id === loadedClip) return;
    loadedClip = id;
    opts = { downbeat_shift: 0, tempo_factor: 1, beats_per_bar: null, no_pickup: null };
    void load();
    api.clipPeaks(id, 2400)
      .then((r) => (peaks = r.peaks))
      .catch(() => (peaks = []));
  });

  function shiftDownbeat(d: number) {
    opts.downbeat_shift += d;
    void load();
  }
  function setFactor(f: number) {
    opts.tempo_factor = f;
    void load();
  }
  function setMeter(n: number | null) {
    opts.beats_per_bar = n;
    void load();
  }
  function setPickup(none: boolean) {
    opts.no_pickup = none;
    void load();
  }

  async function applyGrid() {
    if (!clipId) return;
    applying = true;
    try {
      await api.reconApplyGrid(clipId, $state.snapshot(opts), withSections);
      showToast("ok", tr("曲のテンポと拍子を参考曲に合わせました(取り消しで戻せます)", "Fitted the song's tempo and meter to the reference (undo to revert)"));
      step = 2;
    } catch (e) {
      showError(tr("格子を曲に使えませんでした", "Couldn't apply the grid"), e);
    } finally {
      applying = false;
    }
  }

  // ---- 波形と格子の絵 ----
  const PX_PER_SEC = 40;
  const H = 150;
  let canvas = $state<HTMLCanvasElement | null>(null);
  const width = $derived(Math.max(600, Math.ceil((ws?.duration_sec ?? 60) * PX_PER_SEC)));

  $effect(() => {
    const c = canvas;
    const w = ws;
    const pk = peaks;
    if (!c) return;
    const dpr = window.devicePixelRatio || 1;
    c.width = Math.round(width * dpr);
    c.height = Math.round(H * dpr);
    const g = c.getContext("2d");
    if (!g) return;
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    g.clearRect(0, 0, width, H);
    const top = 22;
    const mid = top + (H - top) / 2;
    const half = (H - top) / 2 - 4;
    // 波形
    if (pk.length && w) {
      g.fillStyle = color("--reference", 0.55, c);
      const per = width / pk.length;
      for (let i = 0; i < pk.length; i++) {
        const [lo, hi] = pk[i];
        const y0 = mid - hi * half;
        const y1 = mid - lo * half;
        g.fillRect(i * per, y0, Math.max(1, per), Math.max(1, y1 - y0));
      }
    }
    if (!w) return;
    // 弱起なしで切る所(1 小節目より前)は暗くする(曲に入らない)
    if (w.no_pickup && w.bars.length && w.bars[0].time > 0) {
      g.fillStyle = color("--bg", 0.6, c);
      g.fillRect(0, 0, w.bars[0].time * PX_PER_SEC, H);
    }
    // 拍(細い線)
    g.strokeStyle = color("--text-dim", 0.35, c);
    g.lineWidth = 1;
    for (const b of w.beats) {
      const x = Math.round(b * PX_PER_SEC) + 0.5;
      g.beginPath();
      g.moveTo(x, top);
      g.lineTo(x, H);
      g.stroke();
    }
    // 小節線と番号・和音
    g.font = "11px sans-serif";
    g.textBaseline = "top";
    for (const bar of w.bars) {
      // 弱起の小節の頭が素材の頭より前なら、左端に寄せて番号だけ出す
      const x = Math.round(Math.max(0, bar.time) * PX_PER_SEC) + 0.5;
      g.strokeStyle = color("--accent", 0.9, c);
      g.lineWidth = 1.5;
      g.beginPath();
      g.moveTo(x, top - 4);
      g.lineTo(x, H);
      g.stroke();
      g.fillStyle = color("--accent", 1, c);
      g.fillText(String(bar.bar), x + 3, 2);
      if (bar.chords && bar.chords !== "N.C.") {
        g.fillStyle = color("--text", 0.9, c);
        g.fillText(bar.chords, x + 3, top - 2 + 2);
      }
    }
  });

  // ---- 再生位置(曲の tick → 参考曲の素材の秒)と、波形を押して移動 ----
  const refClip = $derived.by(() => {
    const c = refTrack?.clips.find((c) => c.kind === "audio");
    return c && c.kind === "audio" ? c : undefined;
  });
  const assetRate = $derived(
    refClip ? ((project.assets?.[refClip.asset] as { sample_rate?: number } | undefined)?.sample_rate ?? 48000) : 48000,
  );
  const ppq = $derived(project.ppq ?? 960);
  /** 曲の tick → 素材の秒(クリップの外も素材の時間の延長として) */
  function audioSecAt(tick: number): number {
    if (!refClip) return 0;
    const songSec = tickToSeconds(project.tempo_map, tick, ppq);
    const clipSec = tickToSeconds(project.tempo_map, refClip.start, ppq);
    return songSec - clipSec + (refClip.offset_samples ?? 0) / assetRate;
  }
  const playX = $derived(audioSecAt(playheadTick) * PX_PER_SEC);
  let scroller = $state<HTMLDivElement | null>(null);
  // 再生中は再生位置が見えるように横に送る
  $effect(() => {
    const el = scroller;
    const x = playX;
    if (!el || !playing) return;
    if (x < el.scrollLeft + 20 || x > el.scrollLeft + el.clientWidth - 40) {
      el.scrollLeft = Math.max(0, x - el.clientWidth * 0.25);
    }
  });
  function seekAt(e: MouseEvent) {
    if (!refClip || !canvas) return;
    const r = canvas.getBoundingClientRect();
    const sec = (e.clientX - r.left) / PX_PER_SEC;
    const clipSec = tickToSeconds(project.tempo_map, refClip.start, ppq);
    const songSec = sec - (refClip.offset_samples ?? 0) / assetRate + clipSec;
    void api.transportSeek(Math.max(0, Math.round(secondsToTick(project.tempo_map, songSec, ppq))));
  }

  const tempoText = $derived.by(() => {
    if (!ws) return "";
    const bpms = ws.tempo.map((t) => t.bpm);
    const lo = Math.min(...bpms);
    const hi = Math.max(...bpms);
    return Math.abs(hi - lo) < 0.5
      ? `${lo.toFixed(1)} BPM`
      : tr(`${lo.toFixed(0)}〜${hi.toFixed(0)} BPM(${ws.tempo.length} 区間)`, `${lo.toFixed(0)}–${hi.toFixed(0)} BPM (${ws.tempo.length} segments)`);
  });

  // ---- 2. パート ----
  /** chords: 和音のパートを、音符を起こさず推定した和音から組み立てる。role: 電子音楽の役割(役割を見分けて組み立てる) */
  type PartRow = {
    on: boolean;
    name: string;
    kind: api.ReconPart["kind"];
    program: number;
    source: string | null;
    chords: boolean;
    role: api.ReconRole | null;
    label: string;
    hint: string;
  };
  /** パートの組み方: 電子音楽(役割ごと)/ 楽器ごと */
  let layout = $state<"electronic" | "instruments">("electronic");
  // 電子音楽: 楽器の名前ではなく役割で起こす(混ざったシンセの音は名前では見分けられないため)。曲に無い役割はトラックにならない
  const electronicRows = $state<PartRow[]>([
    { on: true, name: tr("ドラム", "Drums"), kind: "drums", program: 0, source: null, chords: false, role: null, label: tr("ドラム", "Drums"), hint: tr("キック・スネア・ハイハット・シンバル", "Kick, snare, hi-hat, cymbal") },
    { on: true, name: tr("ベース", "Bass"), kind: "bass", program: 38, source: null, chords: false, role: null, label: tr("ベース", "Bass"), hint: tr("単音。いちばん低い音", "Monophonic; the lowest line") },
    { on: true, name: tr("パッド", "Pad"), kind: "poly", program: 89, source: null, chords: false, role: "sustain", label: tr("パッド(伸ばす)", "Pad (sustained)"), hint: tr("伸ばす和音・持続音(パッド・ストリングス・ドローン)", "Held chords and drones (pads, strings)") },
    { on: true, name: tr("スタブ", "Stab"), kind: "poly", program: 81, source: null, chords: false, role: "stab", label: tr("スタブ(刻む)", "Stab (rhythmic)"), hint: tr("短く刻む和音", "Short rhythmic chords") },
    { on: true, name: tr("アルペジオ・旋律", "Arp / lead"), kind: "poly", program: 80, source: null, chords: false, role: "line", label: tr("アルペジオ・旋律(線)", "Arp / lead (line)"), hint: tr("単音の分散和音・旋律・ベル", "Single-note arpeggios, leads, bells") },
  ]);
  const instrumentRows = $state<PartRow[]>([
    { on: true, name: tr("歌・旋律", "Melody"), kind: "melody", program: 53, source: null, chords: false, role: null, label: tr("歌・旋律", "Vocal / melody"), hint: tr("単音。いちばん目立つ旋律", "Monophonic; the most prominent line") },
    { on: true, name: tr("ベース", "Bass"), kind: "bass", program: 33, source: null, chords: false, role: null, label: tr("ベース", "Bass"), hint: tr("単音。いちばん低い音", "Monophonic; the lowest line") },
    { on: true, name: tr("ピアノ", "Piano"), kind: "poly", program: 0, source: null, chords: true, role: null, label: tr("ピアノ", "Piano"), hint: tr("和音", "Chords") },
    { on: true, name: tr("ドラム", "Drums"), kind: "drums", program: 0, source: null, chords: false, role: null, label: tr("ドラム", "Drums"), hint: tr("キック・スネア・ハイハット・シンバル", "Kick, snare, hi-hat, cymbal") },
    { on: false, name: tr("ギター", "Guitar"), kind: "poly", program: 25, source: null, chords: false, role: null, label: tr("ギター", "Guitar"), hint: tr("和音", "Chords") },
    { on: false, name: tr("その他", "Other"), kind: "poly", program: 48, source: null, chords: false, role: null, label: tr("その他(和音)", "Other (chords)"), hint: tr("弦・パッドなど", "Strings, pads, etc.") },
  ]);
  const rows = $derived(layout === "electronic" ? electronicRows : instrumentRows);
  let quantize = $state(0.5);
  let transcribing = $state(false);

  // ---- 今の曲に取り込む ----
  let imp = $state<api.ReconImport>({ from_bar: 1, to_bar: 8, at_bar: 1, transpose: null, track_id: null });
  const midiTracks = $derived(project.tracks.filter((t) => t.kind === "midi"));
  const onePart = $derived(rows.filter((r) => r.on).length === 1);
  $effect(() => {
    if (!onePart && imp.track_id) imp.track_id = null;
  });
  async function importParts() {
    if (!clipId) return;
    const parts: api.ReconPart[] = rows.filter((r) => r.on).map((r) => ({ name: r.name, kind: r.kind, program: r.program, source: r.source, from_chords: r.chords, role: r.role }));
    if (!parts.length) return;
    transcribing = true;
    try {
      const res = await api.reconImport(clipId, $state.snapshot(opts), parts, quantize, $state.snapshot(imp));
      const t = res.transpose === 0 ? tr("移調なし", "no transposition") : tr(`${res.transpose > 0 ? "+" : ""}${res.transpose} 半音移調`, `transposed ${res.transpose > 0 ? "+" : ""}${res.transpose} semitones`);
      showToast("ok", resultText(res.parts, tr(`取り込みました(${t})`, `Imported (${t})`)));
      step = 3;
    } catch (e) {
      showError(tr("取り込めませんでした", "Couldn't import"), e);
    } finally {
      transcribing = false;
    }
  }

  // ---- 分離ツール(利用者が入れた Demucs) ----
  let sepAvailable = $state<boolean | null>(null);
  let separating = $state(false);
  $effect(() => {
    if (step !== 2 || sepAvailable !== null) return;
    api.reconSeparatorAvailable()
      .then((v) => (sepAvailable = v))
      .catch(() => (sepAvailable = false));
  });

  async function separate() {
    if (!clipId) return;
    separating = true;
    try {
      const f = await api.reconSeparate(clipId);
      for (const r of [...electronicRows, ...instrumentRows]) {
        if (r.chords) continue;
        r.source = (r.kind === "melody" ? (f.melody ?? f.vocals) : r.kind === "bass" ? f.bass : r.kind === "drums" ? f.drums : f.other) ?? null;
      }
      showToast("ok", tr("参考曲を歌・ドラム・ベース・その他に分けました。それぞれの分けた音から起こします", "Separated the reference into vocals, drums, bass and other; parts will be transcribed from them"));
    } catch (e) {
      showError(tr("分けられませんでした", "Couldn't separate"), e);
    } finally {
      separating = false;
    }
  }

  async function pickSource(r: PartRow) {
    const file = await pickFile({
      title: tr(`「${r.label}」だけを分けた音`, `Audio with only “${r.label}”`),
      filters: [{ name: tr("音声(WAV / MP3 / FLAC / OGG / M4A)", "Audio (WAV / MP3 / FLAC / OGG / M4A)"), extensions: ["wav", "mp3", "flac", "ogg", "m4a", "aac"] }],
    });
    if (typeof file === "string") {
      r.source = file;
      r.chords = false;
    }
  }

  const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;

  /** 起こした結果の知らせ(音の無かったパートは「見つかりませんでした」。トラックは作られない) */
  function resultText(parts: { name: string; notes: number }[], head: string): string {
    const found = parts.filter((p) => p.notes > 0);
    const missing = parts.filter((p) => p.notes === 0);
    let s = `${head}: ${found.map((p) => tr(`${p.name} ${p.notes} 音`, `${p.name} ${plural(p.notes, "note")}`)).join(tr("・", ", ")) || tr("なし", "none")}`;
    if (missing.length) {
      s += tr(
        `。見つからなかったパート(トラックは作りません): ${missing.map((p) => p.name).join("・")}`,
        `. Not found (no track made): ${missing.map((p) => p.name).join(", ")}`,
      );
    }
    return s + tr("(取り消しで戻せます)", " (undo to revert)");
  }

  async function transcribe() {
    if (!clipId) return;
    const parts: api.ReconPart[] = rows.filter((r) => r.on).map((r) => ({ name: r.name, kind: r.kind, program: r.program, source: r.source, from_chords: r.chords, role: r.role }));
    if (!parts.length) return;
    transcribing = true;
    try {
      const res = await api.reconTranscribe(clipId, $state.snapshot(opts), parts, quantize);
      showToast("ok", resultText(res.parts, tr("起こしました", "Transcribed")));
      step = 3;
    } catch (e) {
      showError(tr("起こせませんでした", "Couldn't transcribe"), e);
    } finally {
      transcribing = false;
    }
  }

  onMount(() => {
    if (clipId) void load();
  });
</script>

<div class="recon">
  <div class="rv-head">
    <div class="rv-title">
      <Icon name="audio-waveform" />
      <span>{tr("曲から起こす", "Transcribe a song")}</span>
      <span
        class="rv-exp"
        title={tr(
          "開発中の実験的な機能です。起こしたテンポ・小節・和音・音符には間違いが多く、方式もこれから変わります",
          "An experimental feature in development. The transcribed tempo, bars, chords and notes often contain mistakes, and the method will change",
        )}>{tr("開発中・実験的", "Experimental")}</span
      >
    </div>
    {#if refTrack}
      <span class="rv-ref" title={ws?.summary ?? ""}><Icon name="disc-3" size={14} />{refTrack.name}</span>
      <div class="modes" role="group" aria-label={tr("起こし方", "Mode")}>
        <button
          class="btn sm"
          class:on={mode === "new"}
          aria-pressed={mode === "new"}
          onclick={() => (mode = "new")}
          title={tr("曲のテンポ・拍子を参考曲に合わせて、パートを丸ごと起こす(白紙から作るとき)", "Fit the song's tempo and meter to the reference and transcribe whole parts (starting from scratch)")}
          >{tr("曲を合わせて起こす", "Fit the song")}</button
        >
        <button
          class="btn sm"
          class:on={mode === "import"}
          aria-pressed={mode === "import"}
          onclick={() => (mode = "import")}
          title={tr("今の曲のテンポ・拍子・調のまま、参考曲の一部の小節・パートを取り込む(作曲の途中で)", "Import some bars and parts of the reference into the current song, keeping its tempo, meter and key")}
          >{tr("今の曲に取り込む", "Import into this song")}</button
        >
      </div>
      <div class="steps" role="tablist" aria-label={tr("手順", "Steps")}>
        {#each [[1, tr("格子", "Grid")], [2, tr("パート", "Parts")], [3, tr("確かめる", "Check")]] as [n, label] (n)}
          <button role="tab" class:on={step === n} aria-selected={step === n} onclick={() => (step = n as 1 | 2 | 3)}
            ><span class="num">{n}</span>{label}</button
          >
        {/each}
      </div>
    {/if}
  </div>

  {#if !refTrack || !clipId}
    <div class="empty">
      <p>{tr("まず参考曲を読み込みます。曲の音には混ぜず、テンポ・小節・和音を読み取ってから、パートを起こします。", "First load a reference song. It isn't mixed into your song; its tempo, bars and chords are read, then parts are transcribed.")}</p>
      <p class="dim">
        {tr(
          "手元の音声ファイル(WAV / MP3 / FLAC / OGG / M4A)を使い、処理はこの PC の中だけで行います。起こした譜面を公開するときは、編曲などの許諾が別に要ります。",
          "Uses an audio file on this PC and processes it locally. Publishing a transcription needs separate permission (arrangement rights, etc.).",
        )}
      </p>
      <button class="btn primary" onclick={() => importReferenceFile()}><Icon name="disc-3" />{tr("参考曲を読み込む…", "Load a reference song…")}</button>
    </div>
  {:else if step === 1}
    <div class="pane">
      <p class="lead">
        {tr(
          "推定した小節線(番号の付いた線)と拍(細い線)が、参考曲の音の頭(ドラムの強い拍など)とそろっているか見てください。ずれていれば下のボタンで直します。小節の上の文字は推定した和音です。",
          "Check that the estimated bar lines (numbered) and beats (thin lines) line up with the reference's attacks (e.g. strong drum beats). Fix them with the buttons below if not. The labels above the bars are estimated chords.",
        )}
      </p>
      <div class="wave-scroll" bind:this={scroller}>
        {#if loading && !ws}
          <div class="busy"><Icon name="loader-circle" />{tr("参考曲を解析しています…", "Analyzing the reference…")}</div>
        {/if}
        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
        <canvas
          bind:this={canvas}
          style="width:{width}px;height:{H}px"
          onclick={seekAt}
          title={tr("押すとその位置から再生(上の帯の「参考曲」で参考曲を聴けます)", "Click to play from here (listen to the reference with “Reference” on the top bar)")}
        ></canvas>
        {#if playX >= 0 && playX <= width}
          <div class="playhead" style="left:{playX}px;height:{H}px"></div>
        {/if}
      </div>
      {#if loadErr}<p class="err">{loadErr}</p>{/if}
      {#if ws}
        <dl class="facts">
          <div><dt>{tr("テンポ", "Tempo")}</dt><dd>{tempoText}</dd></div>
          <div><dt>{tr("拍子", "Meter")}</dt><dd>{ws.beats_per_bar}/4</dd></div>
          <div><dt>{tr("調(推定)", "Key (est.)")}</dt><dd>{ws.key?.name ?? "—"}</dd></div>
          <div><dt>{tr("チューニング", "Tuning")}</dt><dd>{ws.tuning_cents >= 0 ? "+" : ""}{ws.tuning_cents.toFixed(0)} {tr("セント", "cents")}</dd></div>
          <div>
            <dt>{tr("弱起", "Pickup")}</dt>
            <dd>
              {#if ws.no_pickup}{tr("なし(最初の小節頭が 1 小節目)", "None (first downbeat is bar 1)")}{:else if ws.lead_beats > 0}{tr(
                  `あり(1 小節目は ${ws.lead_beats % ws.beats_per_bar || ws.beats_per_bar} 拍)`,
                  `Yes (bar 1 has ${ws.lead_beats % ws.beats_per_bar || ws.beats_per_bar} beats)`,
                )}{:else}{tr("なし", "None")}{/if}
            </dd>
          </div>
        </dl>
      {/if}
      <div class="controls">
        <div class="group" role="group" aria-label={tr("小節頭", "Downbeat")}>
          <span class="glabel">{tr("小節頭", "Downbeat")}</span>
          <button class="btn sm" onclick={() => shiftDownbeat(-1)} disabled={loading} title={tr("小節頭を 1 拍前へずらす", "Move the downbeat one beat earlier")}
            >◀ {tr("1 拍前へ", "1 beat earlier")}</button
          >
          <button class="btn sm" onclick={() => shiftDownbeat(1)} disabled={loading} title={tr("小節頭を 1 拍後ろへずらす", "Move the downbeat one beat later")}
            >{tr("1 拍後ろへ", "1 beat later")} ▶</button
          >
        </div>
        <div class="group seg" role="group" aria-label={tr("拍の取り方", "Beat level")}>
          <span class="glabel">{tr("拍の取り方", "Beat level")}</span>
          <button class="btn sm" class:on={opts.tempo_factor < 1} onclick={() => setFactor(0.5)} disabled={loading} title={tr("拍を半分の速さに取り直す(テンポが倍に推定されたとき)", "Take beats at half speed (if the tempo was detected double)")}>{tr("半分", "Half")}</button>
          <button class="btn sm" class:on={opts.tempo_factor === 1} onclick={() => setFactor(1)} disabled={loading} title={tr("推定のまま", "As detected")}>{tr("そのまま", "As is")}</button>
          <button class="btn sm" class:on={opts.tempo_factor > 1} onclick={() => setFactor(2)} disabled={loading} title={tr("拍を倍の速さに取り直す(テンポが半分に推定されたとき)", "Take beats at double speed (if the tempo was detected half)")}>{tr("倍", "Double")}</button>
        </div>
        <div class="group seg" role="group" aria-label={tr("拍子", "Meter")}>
          <span class="glabel">{tr("拍子", "Meter")}</span>
          {#each [3, 4, 6] as n (n)}
            <button class="btn sm" class:on={(opts.beats_per_bar ?? ws?.beats_per_bar) === n} onclick={() => setMeter(n)} disabled={loading} title={tr(`1 小節 ${n} 拍にする`, `${n} beats per bar`)}>{n}/4</button>
          {/each}
        </div>
        <div class="group seg" role="group" aria-label={tr("弱起", "Pickup")}>
          <span class="glabel">{tr("弱起", "Pickup")}</span>
          <button
            class="btn sm"
            class:on={ws?.no_pickup === false}
            onclick={() => setPickup(false)}
            disabled={loading}
            title={tr("最初の小節頭の前の拍を 1 小節目(弱起)にする", "Treat the beats before the first downbeat as bar 1 (pickup)")}>{tr("あり", "Yes")}</button
          >
          <button
            class="btn sm"
            class:on={ws?.no_pickup === true}
            onclick={() => setPickup(true)}
            disabled={loading}
            title={tr("最初の小節頭を 1 小節目にする(その前の音は曲に入れない)", "Make the first downbeat bar 1 (audio before it is left out)")}>{tr("なし", "No")}</button
          >
        </div>
        <label class="check"><input type="checkbox" bind:checked={withSections} />{tr("区間の印(イントロ・サビなど)も置く", "Also place section markers (intro, chorus…)")}</label>
        {#if mode === "new"}
          <button class="btn primary go" onclick={applyGrid} disabled={!ws || applying || loading}
            >{#if applying}<Icon name="loader-circle" />{:else}<Icon name="check" />{/if}{tr("この格子を曲に使う", "Use this grid for the song")}</button
          >
        {:else}
          <button
            class="btn primary go"
            onclick={() => (step = 2)}
            disabled={!ws || loading}
            title={tr("今の曲のテンポ・拍子は変えません。小節の番号を取り込む範囲の指定に使います", "The song's tempo and meter stay as they are; bar numbers are used to choose what to import")}
            ><Icon name="check" />{tr("この格子で次へ", "Next with this grid")}</button
          >
        {/if}
      </div>
    </div>
  {:else if step === 2}
    <div class="pane">
      <p class="lead">
        {tr(
          "起こすパートを選びます。参考曲そのままから起こすと、ほかの楽器の音も拾います。そのパートだけを分けた音(分離ツールで作ったファイル)があれば選ぶと確かになります。",
          "Choose the parts to transcribe. Transcribing from the full reference also picks up other instruments; choosing audio with only that part (made with a separation tool) is more accurate.",
        )}
      </p>
      <div class="sep-box">
        {#if sepAvailable === null}
          <span class="dim">{tr("分離ツールを確かめています…", "Checking for a separation tool…")}</span>
        {:else if sepAvailable}
          <button class="btn" onclick={separate} disabled={separating}
            >{#if separating}<Icon name="loader-circle" />{tr("分けています…(曲の長さによって数分かかります)", "Separating… (may take a few minutes)")}{:else}<Icon
                name="split"
              />{tr("分離ツール(Demucs)で分ける", "Separate with Demucs")}{/if}</button
          >
          <span class="dim">{tr("歌・ドラム・ベース・その他に分けて、それぞれから起こします(精度が大きく上がります)", "Splits into vocals, drums, bass and other and transcribes each (much more accurate)")}</span>
        {:else}
          <p class="dim">
            {tr(
              "分けた音から起こすと、精度が大きく上がります。分離ツールの Demucs をこの PC に入れると、ここから分けられます(Python で pip install demucs)。Demucs の学習済みの重みは、作者が研究目的での利用としています。条件を確かめてから使ってください。ほかの分離ツール(UVR など)で作ったファイルを「分けた音…」で選ぶこともできます。",
              "Transcribing from separated audio is much more accurate. Install the Demucs separation tool on this PC (pip install demucs in Python) to separate from here. Its trained weights are released by the authors for research use; check the terms before using them. You can also choose files made with other tools (e.g. UVR) under “Separated audio…”.",
            )}
          </p>
        {/if}
      </div>
      <div class="modes layout" role="group" aria-label={tr("パートの組み方", "How to split the parts")}>
        <button
          class="btn sm"
          class:on={layout === "electronic"}
          aria-pressed={layout === "electronic"}
          onclick={() => (layout = "electronic")}
          title={tr(
            "電子音楽向け: 楽器の名前ではなく役割(伸ばす・刻む・線)を曲から見分けて起こします。曲に無い役割はトラックになりません",
            "For electronic music: detects roles (sustained, rhythmic, line) instead of instrument names. Roles not in the song get no track",
          )}>{tr("電子音楽(役割ごと)", "Electronic (by role)")}</button
        >
        <button
          class="btn sm"
          class:on={layout === "instruments"}
          aria-pressed={layout === "instruments"}
          onclick={() => (layout = "instruments")}
          title={tr("歌・ベース・ピアノ・ギターなど、楽器ごとに起こします", "Transcribe by instrument: vocals, bass, piano, guitar…")}
          >{tr("楽器ごと", "By instrument")}</button
        >
      </div>
      <table class="parts">
        <thead>
          <tr><th></th><th>{tr("パート", "Part")}</th><th>{tr("音の素", "Source")}</th><th>{tr("トラックの名前", "Track name")}</th></tr>
        </thead>
        <tbody>
          {#each rows as r (r.kind + r.program + (r.role ?? ""))}
            <tr class:off={!r.on}>
              <td><input type="checkbox" bind:checked={r.on} aria-label={r.label} /></td>
              <td><span class="pname">{r.label}</span><small>{r.hint}</small></td>
              <td>
                <div class="src">
                  <button
                    class="btn sm"
                    class:on={!r.source && !r.chords}
                    onclick={() => {
                      r.source = null;
                      r.chords = false;
                    }}
                    disabled={!r.on}
                    title={tr("参考曲そのものから起こす", "Transcribe from the full reference")}>{tr("参考曲そのまま", "Full reference")}</button
                  >
                  <button class="btn sm" class:on={!!r.source && !r.chords} onclick={() => pickSource(r)} disabled={!r.on} title={r.source ?? tr("このパートだけを分けた音のファイルを選ぶ", "Choose a file with only this part")}
                    ><Icon name="file-audio" size={13} />{r.source ? baseName(r.source) : tr("分けた音…", "Separated audio…")}</button
                  >
                  {#if r.kind === "poly" && !r.role}
                    <button
                      class="btn sm"
                      class:on={r.chords}
                      onclick={() => (r.chords = true)}
                      disabled={!r.on}
                      title={tr(
                        "音符を起こさず、推定した和音(上の和音の並び)から積んで小節ごとに鳴らす。音色が複雑な曲でも和声が合いやすい",
                        "Build the part from the estimated chords instead of transcribing notes (stacked, played each bar). Keeps the harmony right even with complex sounds",
                      )}>{tr("和音から組み立てる", "Build from chords")}</button
                    >
                  {/if}
                </div>
              </td>
              <td><input class="tname" bind:value={r.name} disabled={!r.on} aria-label={tr("トラックの名前", "Track name")} /></td>
            </tr>
          {/each}
        </tbody>
      </table>
      {#if mode === "import"}
        <div class="import-box">
          <label
            >{tr("参考曲の", "Reference bars")}
            <input type="number" min="1" bind:value={imp.from_bar} aria-label={tr("参考曲の最初の小節", "First reference bar")} />〜<input
              type="number"
              min="1"
              bind:value={imp.to_bar}
              aria-label={tr("参考曲の最後の小節", "Last reference bar")}
            />{tr("小節を", "")}</label
          >
          <label
            >{tr("今の曲の", "to bar")}
            <input type="number" min="1" bind:value={imp.at_bar} aria-label={tr("置き始める小節", "Bar to place at")} />{tr("小節目から", "of this song")}</label
          >
          <label
            >{tr("移調", "Transpose")}
            <select
              value={imp.transpose === null ? "auto" : String(imp.transpose)}
              onchange={(e) => {
                const v = (e.currentTarget as HTMLSelectElement).value;
                imp.transpose = v === "auto" ? null : Number(v);
              }}
              aria-label={tr("移調", "Transpose")}
            >
              <option value="auto">{tr("今の曲の調へ(自動)", "To this song's key (auto)")}</option>
              {#each [-6, -5, -4, -3, -2, -1, 0, 1, 2, 3, 4, 5, 6] as n (n)}
                <option value={String(n)}>{n === 0 ? tr("そのまま", "None") : `${n > 0 ? "+" : ""}${n}`}</option>
              {/each}
            </select></label
          >
          <label
            >{tr("置き先", "Destination")}
            <select
              value={imp.track_id ?? ""}
              onchange={(e) => {
                const v = (e.currentTarget as HTMLSelectElement).value;
                imp.track_id = v || null;
              }}
              disabled={!onePart}
              title={onePart ? "" : tr("パートを 1 つにすると、今あるトラックに置けます", "Choose one part to place it on an existing track")}
              aria-label={tr("置き先", "Destination")}
            >
              <option value="">{tr("新しいトラック", "New track")}</option>
              {#each midiTracks as t (t.id)}
                <option value={t.id}>{t.name}</option>
              {/each}
            </select></label
          >
          <span class="dim">{tr("今の曲のテンポ・拍子は変えず、拍の位置で写します", "The song's tempo and meter stay; notes are placed by beat position")}</span>
        </div>
      {/if}
      <div class="controls">
        <div class="group seg" role="group" aria-label={tr("格子への寄せ方", "Snap to grid")}>
          <span class="glabel">{tr("16 分の格子へ", "Snap to 16ths")}</span>
          <button class="btn sm" class:on={quantize === 0} onclick={() => (quantize = 0)} title={tr("演奏の位置のまま", "Keep the played timing")}>{tr("寄せない", "None")}</button>
          <button class="btn sm" class:on={quantize === 0.5} onclick={() => (quantize = 0.5)} title={tr("半分だけ寄せる(揺れを少し残す)", "Halfway (keeps some feel)")}>{tr("半分", "Half")}</button>
          <button class="btn sm" class:on={quantize === 1} onclick={() => (quantize = 1)} title={tr("格子にそろえる", "Fully on the grid")}>{tr("そろえる", "Full")}</button>
        </div>
        {#if mode === "new"}
          <button class="btn primary go" onclick={transcribe} disabled={transcribing || !rows.some((r) => r.on)}
            >{#if transcribing}<Icon name="loader-circle" />{tr("起こしています…", "Transcribing…")}{:else}<Icon name="wand-sparkles" />{tr("起こす", "Transcribe")}{/if}</button
          >
        {:else}
          <button class="btn primary go" onclick={importParts} disabled={transcribing || !rows.some((r) => r.on) || imp.to_bar < imp.from_bar}
            >{#if transcribing}<Icon name="loader-circle" />{tr("取り込んでいます…", "Importing…")}{:else}<Icon name="wand-sparkles" />{tr("取り込む", "Import")}{/if}</button
          >
        {/if}
      </div>
    </div>
  {:else}
    <div class="pane">
      <p class="lead">
        {tr(
          "起こしたパートはトラックとして入りました。参考曲と重ねたり、遅くしたりして聴き比べ、タイムラインとピアノロールで直します。",
          "The transcribed parts are now tracks. Compare with the reference by overlaying or slowing down, and fix them in the timeline and piano roll.",
        )}
      </p>
      <div class="controls">
        <button class="btn" class:on={referenceStore.mode === "both"} onclick={() => setReferenceMode(referenceStore.mode === "both" ? "song" : "both", projectVersion)}
          ><Icon name="layers" />{tr("参考曲と重ねて聴く", "Overlay the reference")}</button
        >
        <button class="btn" class:on={referenceStore.mode === "reference"} onclick={() => setReferenceMode(referenceStore.mode === "reference" ? "song" : "reference", projectVersion)}
          ><Icon name="disc-3" />{tr("参考曲だけ", "Reference only")}</button
        >
        <div class="group seg" role="group" aria-label={tr("再生の速さ", "Playback speed")}>
          <span class="glabel">{tr("速さ", "Speed")}</span>
          {#each [1, 0.75, 0.5] as sp (sp)}
            <button class="btn sm" class:on={referenceStore.speed === sp} onclick={() => setSpeed(sp)}>{Math.round(sp * 100)}%</button>
          {/each}
        </div>
        <button class="btn primary" onclick={() => (viewStore.main = "timeline")}><Icon name="rows-2" />{tr("タイムラインで直す", "Edit in the timeline")}</button>
      </div>
      <p class="dim">
        {tr(
          "ルーラーで範囲を選んでループ(L)にすると、その区間を繰り返し聴けます。",
          "Select a range on the ruler and turn on Loop (L) to repeat it.",
        )}
      </p>
      {#if ws}
        <h3>{tr("小節ごとの和音(推定)", "Chords per bar (estimated)")}</h3>
        <div class="chart">
          {#each ws.bars as b (b.bar)}
            <div class="cell"><span class="bn">{b.bar}</span><span class="ch">{b.chords === "N.C." ? "—" : b.chords}</span></div>
          {/each}
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .recon {
    position: absolute;
    inset: 0;
    overflow: auto;
    padding: var(--sp-3) var(--sp-4);
    background: var(--bg);
    color: var(--text);
  }

  .rv-head {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    flex-wrap: wrap;
    margin-bottom: var(--sp-3);
  }

  .rv-title {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--fs-lg);
    font-weight: 600;
  }

  /* 開発中の実験的な機能であることの但し書き */
  .rv-exp {
    padding: 1px 6px;
    border: 1px solid var(--warn);
    border-radius: var(--r-sm);
    color: var(--warn);
    font-size: var(--fs-xs);
    font-weight: 500;
    white-space: nowrap;
  }

  .rv-ref {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    color: var(--reference);
    font-size: var(--fs-sm);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 320px;
  }

  .steps {
    display: flex;
    margin-left: auto;
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    overflow: hidden;
  }

  .steps button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 5px 14px;
    border: 0;
    border-radius: 0;
    background: none;
    color: var(--text-dim);
    font-size: var(--fs-sm);
  }

  .steps button + button {
    border-left: 1px solid var(--border);
  }

  .steps button.on {
    background: color-mix(in srgb, var(--accent) 14%, var(--bg-panel));
    color: var(--accent);
  }

  .num {
    display: inline-grid;
    place-items: center;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    border: 1px solid currentColor;
    font-size: var(--fs-xs);
  }

  .empty {
    max-width: 560px;
    margin: 48px auto;
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
    align-items: flex-start;
  }

  .pane {
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
  }

  .lead {
    margin: 0;
    color: var(--text);
    font-size: var(--fs-sm);
  }

  .dim {
    margin: 0;
    color: var(--text-dim);
    font-size: var(--fs-sm);
  }

  .err {
    color: var(--danger-text);
    font-size: var(--fs-sm);
  }

  .wave-scroll {
    position: relative;
    overflow-x: auto;
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    background: var(--bg-inset);
  }

  .wave-scroll canvas {
    display: block;
    cursor: pointer;
  }

  .playhead {
    position: absolute;
    top: 0;
    width: 2px;
    margin-left: -1px;
    background: var(--text);
    pointer-events: none;
  }

  .busy {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    color: var(--text-dim);
    font-size: var(--fs-sm);
  }

  .facts {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-4);
    margin: 0;
  }

  .facts div {
    display: flex;
    flex-direction: column;
  }

  .facts dt {
    color: var(--text-dim);
    font-size: var(--fs-xs);
  }

  .facts dd {
    margin: 0;
    font-size: var(--fs-md);
    font-variant-numeric: tabular-nums;
  }

  .controls {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--sp-3);
  }

  .group {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .glabel {
    color: var(--text-dim);
    font-size: var(--fs-xs);
    margin-right: 2px;
  }

  .check {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: var(--fs-sm);
    color: var(--text);
  }

  .go {
    margin-left: auto;
  }

  .modes {
    display: flex;
    gap: 4px;
  }

  .modes.layout {
    margin: 4px 0 8px;
  }

  .import-box {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--sp-3);
    font-size: var(--fs-sm);
  }

  .import-box label {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }

  .import-box input[type="number"] {
    width: 64px;
  }

  .sep-box {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--sp-2);
    padding: var(--sp-2) var(--sp-3);
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    background: var(--bg-panel);
  }

  .parts {
    /* 縦に並べる欄の中で横に伸ばされないように */
    align-self: flex-start;
    border-collapse: collapse;
    font-size: var(--fs-sm);
  }

  .parts th {
    text-align: left;
    color: var(--text-dim);
    font-weight: normal;
    font-size: var(--fs-xs);
    padding: 4px 8px;
  }

  .parts td {
    padding: 6px 8px;
    border-top: 1px solid var(--border);
    vertical-align: middle;
  }

  .parts tr.off td {
    color: var(--text-dim);
  }

  .pname {
    display: block;
  }

  .parts small {
    color: var(--text-dim);
    font-size: var(--fs-xs);
  }

  .src {
    display: flex;
    gap: 4px;
  }

  .src .btn {
    max-width: 220px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .tname {
    width: 140px;
  }

  h3 {
    margin: var(--sp-2) 0 0;
    font-size: var(--fs-md);
    font-weight: 600;
  }

  .chart {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(92px, 1fr));
    gap: 4px;
  }

  .cell {
    display: flex;
    align-items: baseline;
    gap: 6px;
    padding: 4px 6px;
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    background: var(--bg-panel);
    min-width: 0;
  }

  .bn {
    color: var(--text-faint);
    font-size: var(--fs-xs);
  }

  .ch {
    font-size: var(--fs-sm);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
