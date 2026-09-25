<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "./lib/api";
  import { shouldYieldKey } from "./lib/keys";
  import { toolDoing } from "./lib/toolLabels";
  import Toasts from "./lib/Toasts.svelte";
  import { harmonyStore, refreshHarmony } from "./lib/harmony.svelte";
  import { barAtTick, buildBars, nextBarHead, prevBarHead } from "./lib/barMap";
  import type { AppInfo, EntrySummary, Project } from "./lib/types";
  import { pollTransport, startTransportPolling, transportStore } from "./lib/transport.svelte";
  import Timeline from "./lib/Timeline.svelte";
  import HistoryPanel from "./lib/HistoryPanel.svelte";
  import ChatPanel from "./lib/ChatPanel.svelte";
  import ProjectMenu from "./lib/ProjectMenu.svelte";
  import PianoRoll from "./lib/PianoRoll.svelte";
  import SettingsPanel from "./lib/SettingsPanel.svelte";
  import SoundDesignPanel from "./lib/SoundDesignPanel.svelte";
  import { applyTheme, settings } from "./lib/settings.svelte";
  import { chatStatus } from "./lib/aiStatus.svelte";
  import {
    MASTER_FOCUS_ID,
    midiArmStore,
    pianoRollStore,
    selectionStore,
    soundDesignStore,
  } from "./lib/selection.svelte";

  let project = $state<Project | null>(null);
  let projectVersion = $state(0);
  let entries = $state<EntrySummary[]>([]);
  let historyTotal = $state(0);
  let redoable = $state<EntrySummary[]>([]);
  let info = $state<AppInfo | null>(null);
  let error = $state<string | null>(null);
  let mcpCopied = $state(false);
  /** 保存に失敗したときのエラー(次の保存が成功すると消える) */
  let saveError = $state<string | null>(null);

  // 再生状態は共有ストア(問い合わせは startTransportPolling の 1 か所だけ)
  const transport = $derived(transportStore.state);
  let showSettings = $state(false);

  function closeSettings() {
    showSettings = false;
    refreshAudioDev();
  }
  let editingBpm = $state(false);
  let bpmInput = $state("");

  // レイアウト(ドラッグで調整、localStorage に保存)
  function loadNum(key: string, fallback: number): number {
    try {
      const v = Number(localStorage.getItem(key));
      return Number.isFinite(v) && v > 0 ? v : fallback;
    } catch {
      return fallback;
    }
  }
  let bottomHeight = $state(loadNum("glaux.bottomHeight", 280));
  /// 下のパネル(チャット・履歴)を隠しているか(狭い画面でピアノロールを広く使う)
  let bottomCollapsed = $state(loadNum("glaux.bottomCollapsed", 0) === 1);

  function toggleBottom() {
    bottomCollapsed = !bottomCollapsed;
    try {
      localStorage.setItem("glaux.bottomCollapsed", bottomCollapsed ? "1" : "0");
    } catch {
      // 保存できなくても動作には関係しない
    }
  }
  let chatFrac = $state(loadNum("glaux.chatFrac", 0.6));

  function saveLayout() {
    try {
      localStorage.setItem("glaux.bottomHeight", String(bottomHeight));
      localStorage.setItem("glaux.chatFrac", String(chatFrac));
    } catch {
      // localStorage が使えなくても動作に支障なし
    }
  }

  function startRowResize(e: PointerEvent) {
    const startY = e.clientY;
    const startH = bottomHeight;
    const move = (ev: PointerEvent) => {
      bottomHeight = Math.min(Math.max(startH + (startY - ev.clientY), 140), window.innerHeight - 200);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      saveLayout();
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  function startColResize(e: PointerEvent) {
    const container = (e.currentTarget as HTMLElement).parentElement!;
    const rect = container.getBoundingClientRect();
    const move = (ev: PointerEvent) => {
      chatFrac = Math.min(Math.max((ev.clientX - rect.left) / rect.width, 0.25), 0.8);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      saveLayout();
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }
  // AI の作業表示:
  //   calling = ツール実行中(明るく点滅)
  //   session = 呼び出しの合間(考え中。次の呼び出しが来なければ一定時間で消灯)
  // アプリからは「指示を出した瞬間」は見えないため、最初のツール呼び出し〜
  // 最後の呼び出し + SESSION_LINGER_MS を「AI の作業時間」として近似する。
  type AiState = "idle" | "calling" | "session";
  const SESSION_LINGER_MS = 20_000;
  let aiState = $state<AiState>("idle");
  let aiTool = $state("");

  // チャット実行中はターン境界が正確に分かるので、MCP ベースの近似より優先して
  // インジケータを点灯し続ける(ターミナル等の外部 MCP クライアントは近似のまま)
  const indicator = $derived<AiState>(
    aiState === "calling"
      ? "calling"
      : aiState === "session" || chatStatus.running
        ? "session"
        : "idle",
  );


  async function refresh() {
    try {
      const [p, h] = await Promise.all([api.getProject(), api.getHistory()]);
      project = p.project;
      projectVersion = p.project_version;
      entries = h.entries;
      historyTotal = h.total;
      refreshHarmony();
      redoable = h.redoable ?? [];
      error = null;
      // プロジェクトの移動・切り替えでパスが変わることがあるのでフッターも更新
      api.appInfo().then((i) => (info = i)).catch(() => {});
    } catch (e) {
      error = String(e);
    }
  }

  // 変更イベントの合流: 先頭は即時、連続分は 80ms 窓でまとめて 1 回だけ再取得する
  // (AI の連続編集で全量再取得が毎回走るのを防ぐ)
  let refreshTimer: ReturnType<typeof setTimeout> | undefined;
  let refreshPending = false;

  function scheduleRefresh() {
    if (refreshTimer) {
      refreshPending = true;
      return;
    }
    refresh();
    refreshTimer = setTimeout(() => {
      refreshTimer = undefined;
      if (refreshPending) {
        refreshPending = false;
        scheduleRefresh();
      }
    }, 80);
  }

  // 使用中のオーディオデバイス(フッター表示用)
  let audioDev = $state<{
    output: string | null;
    input: string | null;
    rate: number;
    midi: string | null;
  } | null>(null);
  async function refreshAudioDev() {
    try {
      const d = await api.audioDevices();
      const m = await api.midiInputs().catch(() => ({ current: null }));
      audioDev = {
        output: d.current_output,
        input: d.current_input,
        rate: d.sample_rate,
        midi: m.current,
      };
    } catch {
      // オーディオが使えない環境
    }
  }

  // MIDI キーボードの送り先: アームしたトラック → ピアノロールで開いているトラック →
  // 最初の MIDI トラック。変わったときだけエンジンへ伝える
  const liveTarget = $derived.by(() => {
    const tracks = project?.tracks ?? [];
    const isMidi = (id: string | null | undefined) =>
      !!id && tracks.some((t) => t.id === id && t.kind === "midi");
    if (isMidi(midiArmStore.trackId)) return midiArmStore.trackId;
    if (isMidi(pianoRollStore.focus?.trackId)) return pianoRollStore.focus!.trackId;
    return tracks.find((t) => t.kind === "midi")?.id ?? null;
  });
  let sentLiveTarget: string | null | undefined = undefined;
  $effect(() => {
    const target = liveTarget;
    if (!transport.available || target === sentLiveTarget) return;
    sentLiveTarget = target;
    api.setLiveTarget(target).catch(() => {});
  });
  // アームしたトラックが消えたら解除
  $effect(() => {
    const id = midiArmStore.trackId;
    if (id && project && !project.tracks.some((t) => t.id === id)) midiArmStore.trackId = null;
  });

  onMount(() => {
    applyTheme();
    api.appInfo().then((i) => (info = i));
    refresh();
    // 前回選んだオーディオデバイスに戻す(抜かれていたら既定のまま)
    (async () => {
      if (settings.outputDevice) {
        await api.setOutputDevice(settings.outputDevice).catch(() => {
          error = `前回の出力デバイス「${settings.outputDevice}」が見つからないため、既定のデバイスを使います`;
        });
      }
      if (settings.inputDevice) {
        await api.setInputDevice(settings.inputDevice).catch(() => {});
      }
      if (settings.midiInput) {
        await api.setMidiInput(settings.midiInput).catch(() => {
          recordNotice = `前回の MIDI 入力「${settings.midiInput}」が見つかりません(接続して設定で選び直してください)`;
        });
      }
      refreshAudioDev();
    })();

    let idleTimer: ReturnType<typeof setTimeout> | undefined;
    const unlistenChanged = api.onProjectChanged((ev) => {
      // 保存の失敗は、次に保存が成功するまで出し続ける(再取得で消える error とは分ける)
      saveError = ev?.save_error ?? null;
      scheduleRefresh();
    }).catch((e) => {
      error = `変更イベントの購読に失敗: ${e}`;
      return undefined;
    });
    const unlistenActivity = api
      .onAiActivity((a) => {
        aiTool = a.tool;
        if (idleTimer) clearTimeout(idleTimer);
        if (a.busy) {
          aiState = "calling";
        } else {
          aiState = "session";
          idleTimer = setTimeout(() => (aiState = "idle"), SESSION_LINGER_MS);
        }
      })
      .catch((e) => {
        error = `AI イベントの購読に失敗: ${e}`;
        return undefined;
      });

    // 保険: ウィンドウにフォーカスが戻ったら取得し直す
    const onFocus = () => refresh();
    window.addEventListener("focus", onFocus);

    // 再生ヘッドのポーリング(再生中 100ms / 停止中は 400ms に間引く)
    const stopTransportPolling = startTransportPolling();

    // キーボード操作(入力欄にフォーカスがあるときは除く)
    // Space: 再生/一時停止, ←/→: 前/次の小節頭, Home/End: 先頭/終端
    const onKeydown = (e: KeyboardEvent) => {
      if (e.key === "Escape" && showSettings) {
        e.preventDefault();
        closeSettings();
        return;
      }
      if (shouldYieldKey(e)) return;
      if ((e.ctrlKey || e.metaKey) && e.code === "KeyZ") {
        e.preventDefault();
        if (e.shiftKey) doRedo();
        else doUndo();
        return;
      }
      if ((e.ctrlKey || e.metaKey) && e.code === "KeyY") {
        e.preventDefault();
        doRedo();
        return;
      }
      switch (e.code) {
        case "Space":
          e.preventDefault();
          togglePlay();
          break;
        case "KeyL":
          if (e.ctrlKey || e.metaKey || e.altKey) break;
          // ピアノロールを開いているときは奏法キーと同系統の扱いにせず、そのままトグル
          e.preventDefault();
          toggleLoop();
          break;
        case "ArrowLeft":
          // ピアノロール表示中は挿入カーソル移動(PianoRoll 側)に譲る
          if (pianoRollStore.focus) return;
          e.preventDefault();
          prevBar();
          break;
        case "ArrowRight":
          if (pianoRollStore.focus) return;
          e.preventDefault();
          nextBar();
          break;
        case "Home":
          e.preventDefault();
          seekStart();
          break;
        case "End":
          e.preventDefault();
          seekEnd();
          break;
      }
    };
    window.addEventListener("keydown", onKeydown);

    return () => {
      unlistenChanged.then((f) => f && f());
      unlistenActivity.then((f) => f && f());
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("keydown", onKeydown);
      stopTransportPolling();
      if (idleTimer) clearTimeout(idleTimer);
    };
  });

  async function togglePlay() {
    if (!transport.available) return;
    try {
      if (transport.playing) {
        await api.transportPause();
      } else {
        await api.transportPlay();
      }
      await pollTransport();
    } catch (e) {
      error = String(e);
    }
  }

  async function stopPlayback() {
    if (!transport.available) return;
    try {
      if (transport.recording) {
        await finishRecording();
        return;
      }
      await api.transportStop();
      await pollTransport();
    } catch (e) {
      error = String(e);
    }
  }

  // ---- オーディオ負荷の表示(音切れの原因切り分け用) ----
  // 平均はポーリングごとの値、最大は直近 3 秒、回数は再生開始からの差分
  let dspAvg = $state(0);
  let dspMaxWindow: number[] = [];
  let dspMax = $state(0);
  let dspBase: { overruns: number; late: number; swaps: number } | null = null;
  let dspCounts = $state({ overruns: 0, late: 0, swaps: 0 });
  $effect(() => {
    const d = transport.dsp;
    if (!d) return;
    if (!transport.playing) {
      dspBase = null;
      return;
    }
    if (!dspBase) dspBase = { overruns: d.overruns, late: d.late, swaps: d.swaps };
    dspAvg = d.avg_pct;
    dspMaxWindow = [...dspMaxWindow.slice(-29), d.max_pct];
    dspMax = Math.max(...dspMaxWindow);
    dspCounts = {
      overruns: d.overruns - dspBase.overruns,
      late: d.late - dspBase.late,
      swaps: d.swaps - dspBase.swaps,
    };
  });
  const dspWarn = $derived(dspCounts.overruns + dspCounts.late > 0 || dspMax > 80);

  // 録音中の入力レベル(上がるときは即座に、下がるときはゆっくり)
  let recLevel = $state(-90);
  $effect(() => {
    const db = transport.input_peak_db;
    if (!transport.recording) {
      recLevel = -90;
      return;
    }
    recLevel = Math.max(db ?? -90, recLevel - 6);
  });

  let recordNotice = $state<string | null>(null);
  /// 直前に録音したクリップ(「♪ MIDI 化」ボタンの対象)
  let lastRecorded = $state<{ clipId: string; trackId: string } | null>(null);
  let recordNoticeTimer: ReturnType<typeof setTimeout> | undefined;

  async function finishMidiRecording() {
    const r = await api.midiRecordStop(midiArmStore.trackId, settings.midiQuantize);
    await pollTransport();
    recordNotice = `MIDI 録音を配置しました: ${r.notes} ノート`;
    clearTimeout(recordNoticeTimer);
    recordNoticeTimer = setTimeout(() => (recordNotice = null), 8000);
  }

  async function finishRecording() {
    const r = await api.recordStop(null, settings.autoGain);
    await pollTransport();
    const warn =
      r.clipped > 0 ? `(${r.clipped} サンプルがクリップしました。入力レベルを下げてください)` : "";
    const gain = r.gain_db > 0.5 ? `、音量 +${r.gain_db.toFixed(0)}dB` : "";
    recordNotice = `録音を配置しました: ${r.seconds.toFixed(1)} 秒${gain}${warn}`;
    lastRecorded = { clipId: r.clip_id, trackId: r.track_id };
    clearTimeout(recordNoticeTimer);
    recordNoticeTimer = setTimeout(() => {
      recordNotice = null;
      lastRecorded = null;
    }, 20000);
  }

  /// 録音した鼻歌をそのまま MIDI にしてピアノロールで開く
  async function transcribeLast() {
    const target = lastRecorded;
    if (!target) return;
    try {
      const r = await api.transcribeClip(target.clipId);
      const track = project?.tracks.find((t) => t.id === r.track_id);
      pianoRollStore.focus = {
        clipId: r.clip_id,
        clipName: "録音 (MIDI)",
        trackId: r.track_id,
        trackName: track?.name ?? "MIDI",
        anchorTick: 0,
      };
      recordNotice = `${r.note_count} ノートを MIDI にしました。AI に「キーを確認して整えて」と頼めます`;
      lastRecorded = null;
    } catch (e) {
      error = String(e);
    }
  }

  async function toggleMetronome() {
    if (!transport.available) return;
    try {
      await api.transportSetMetronome(!transport.metronome);
      await pollTransport();
    } catch (e) {
      error = String(e);
    }
  }

  /// 🎹 アーム中のトラック名(⏺ が MIDI 録音になる)
  const midiArm = $derived(
    project?.tracks.find((t) => t.id === midiArmStore.trackId && t.kind === "midi") ?? null,
  );

  /// ⏺: 録音開始 / 停止。停止すると音声トラック(🎹 アーム中は MIDI トラック)に
  /// クリップとして置かれる
  async function toggleRecord() {
    if (!transport.available) return;
    try {
      if (transport.midi_recording) {
        await finishMidiRecording();
      } else if (transport.recording) {
        await finishRecording();
      } else {
        if (midiArm) {
          await api.midiRecordStart({
            countInBars: settings.countInBars,
            metronome: settings.metronomeOnRecord,
          });
        } else {
          await api.recordStart({
            countInBars: settings.countInBars,
            latencyMs: settings.recordLatencyMs,
            metronome: settings.metronomeOnRecord,
            stereo: settings.recordStereo,
          });
        }
        await pollTransport();
        if (settings.countInBars > 0) {
          recordNotice = `カウントイン ${settings.countInBars} 小節のあと録音位置になります`;
          clearTimeout(recordNoticeTimer);
          recordNoticeTimer = setTimeout(() => (recordNotice = null), 5000);
        }
      }
    } catch (e) {
      error = String(e);
    }
  }

  async function seek(tick: number) {
    if (!transport.available) return;
    try {
      await api.transportSeek(tick);
      await pollTransport();
    } catch (e) {
      error = String(e);
    }
  }

  // ---- 小節ナビゲーション(拍子イベントを考慮した小節マップ準拠) ----

  const navBars = $derived.by(() => {
    if (!project) return [];
    let end = 0;
    for (const t of project.tracks) {
      for (const c of t.clips) {
        end = Math.max(end, c.start + c.length);
      }
    }
    return buildBars(project, end, 1, 1);
  });

  /** コンテンツ終端を小節単位に切り上げた tick */
  const contentEndTick = $derived.by(() => {
    if (!project || navBars.length === 0) return 0;
    let end = 0;
    for (const t of project.tracks) {
      for (const c of t.clips) {
        end = Math.max(end, c.start + c.length);
      }
    }
    if (end === 0) return 0;
    const bar = barAtTick(navBars, end - 1);
    return bar.tick + bar.len;
  });

  function seekStart() {
    seek(0);
  }

  function seekEnd() {
    seek(contentEndTick);
  }

  /** 小節の途中なら小節頭へ、頭にいるなら前の小節頭へ */
  function prevBar() {
    if (navBars.length === 0) return;
    seek(prevBarHead(navBars, transport.tick));
  }

  function nextBar() {
    if (navBars.length === 0) return;
    seek(nextBarHead(navBars, transport.tick));
  }

  // ---- ループ再生 ----
  // ルーラーで範囲選択していればその区間、なければ曲全体をループする。
  // ループ中に選択を変えると区間も追従する。

  let loopOn = $state(false);

  // エンジン側の実状態に追従(プロジェクト切り替えでの自動解除など)
  $effect(() => {
    loopOn = transport.loop != null;
  });

  function loopRange(): { start: number; end: number } | null {
    const r = selectionStore.range;
    const start = r ? r.startTick : 0;
    const end = r ? r.endTick : contentEndTick;
    return end > start ? { start, end } : null;
  }

  async function toggleLoop() {
    if (!transport.available) return;
    try {
      if (loopOn) {
        await api.transportClearLoop();
        loopOn = false;
        return;
      }
      const range = loopRange();
      if (!range) return; // 空プロジェクトなど
      await api.transportSetLoop(range.start, range.end);
      loopOn = true;
    } catch (e) {
      error = String(e);
    }
  }

  // ループ中に範囲選択・曲の長さが変わったら区間を更新
  $effect(() => {
    void selectionStore.range;
    void contentEndTick;
    if (!loopOn) return;
    const range = loopRange();
    if (range) api.transportSetLoop(range.start, range.end).catch(() => {});
  });

  let exporting = $state(false);
  let exportMsg = $state<string | null>(null);

  async function doExport() {
    if (exporting) return;
    exporting = true;
    exportMsg = "書き出し中…";
    try {
      const r = await api.exportWav();
      exportMsg = `書き出しました(${r.seconds.toFixed(1)} 秒): ${r.path}`;
    } catch (e) {
      exportMsg = null;
      error = String(e);
    } finally {
      exporting = false;
    }
  }

  function startBpmEdit() {
    bpmInput = String(bpm);
    editingBpm = true;
  }

  async function commitBpm() {
    editingBpm = false;
    if (!project) return;
    const v = Number(bpmInput);
    if (!Number.isFinite(v)) return;
    const clamped = Math.min(300, Math.max(20, v));
    if (Math.abs(clamped - bpm) < 1e-9) return;
    const events = project.tempo_map.map((e, i) =>
      i === 0 ? { ...e, bpm: clamped } : e,
    );
    try {
      await api.applyEdit(
        [{ op: "set_tempo", events }],
        `BPM を ${clamped} に変更`,
      );
    } catch (e) {
      error = String(e);
    }
  }

  function onBpmKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      commitBpm();
    } else if (e.key === "Escape") {
      editingBpm = false;
    }
  }

  // ---- 拍子の編集(先頭イベントの書き換え。途中の変更イベントは保持) ----

  let editingSig = $state(false);
  let sigNumInput = $state(4);
  let sigDenInput = $state(4);

  function startSigEdit() {
    if (!project) return;
    const first = project.time_sig_map[0] ?? { tick: 0, num: 4, den: 4 };
    sigNumInput = first.num;
    sigDenInput = first.den;
    editingSig = true;
  }

  async function commitSig() {
    editingSig = false;
    if (!project) return;
    const num = Math.min(32, Math.max(1, Math.round(Number(sigNumInput)) || 0));
    const den = Number(sigDenInput);
    if (num < 1 || ![1, 2, 4, 8, 16, 32].includes(den)) return;
    const cur = project.time_sig_map;
    const first = cur[0] ?? { tick: 0, num: 4, den: 4 };
    if (first.num === num && first.den === den) return;
    const events =
      cur.length > 0
        ? cur.map((e, i) => (i === 0 ? { ...e, num, den } : e))
        : [{ tick: 0, num, den }];
    try {
      await api.applyEdit(
        [{ op: "set_time_sig", events }],
        `拍子を ${num}/${den} に変更`,
      );
    } catch (e) {
      error = String(e);
    }
  }

  function onSigKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      commitSig();
    } else if (e.key === "Escape") {
      editingSig = false;
    }
  }

  async function setMasterVolume(e: Event) {
    const v = Number((e.currentTarget as HTMLInputElement).value);
    try {
      await api.applyEdit(
        [{ op: "set_master_volume", volume_db: v }],
        `マスター音量を ${v.toFixed(1)} dB に変更`,
      );
    } catch (err) {
      error = String(err);
    }
  }

  async function doUndo() {
    try {
      await api.undo();
    } catch (e) {
      error = String(e);
    }
  }

  async function doRedo() {
    try {
      await api.redo();
    } catch (e) {
      error = String(e);
    }
  }

  async function copyMcpUrl() {
    if (!info) return;
    await navigator.clipboard.writeText(
      `claude mcp add --transport http glaux ${info.mcp_url}`,
    );
    mcpCopied = true;
    setTimeout(() => (mcpCopied = false), 1500);
  }

  const bpm = $derived(project?.tempo_map[0]?.bpm ?? 120);
  const timeSig = $derived(
    project ? `${project.time_sig_map[0]?.num ?? 4}/${project.time_sig_map[0]?.den ?? 4}` : "4/4",
  );
  const hasSigChanges = $derived((project?.time_sig_map.length ?? 0) > 1);
</script>

<div class="layout">
  <header>
    <div class="brand">
      <img class="owl" src="/glaux-icon.png" alt="Glaux" width="28" height="28" />
      <ProjectMenu title={project?.meta.title ?? "…"} />
    </div>
    <div class="transport">
      <button onclick={seekStart} disabled={!transport.available} title="先頭へ(Home)" aria-label="先頭へ">⏮</button>
      <button onclick={prevBar} disabled={!transport.available} title="前の小節頭へ(←)" aria-label="前の小節頭へ">⏪</button>
      <button
        class="play"
        aria-label={transport.playing ? "一時停止" : "再生"}
        onclick={togglePlay}
        disabled={!transport.available}
        title={transport.available
          ? "再生/一時停止(スペースキー)"
          : "オーディオデバイスが利用できません"}
      >
        {transport.playing ? "⏸" : "▶"}
      </button>
      <button onclick={stopPlayback} disabled={!transport.available} title="停止(先頭に戻る)" aria-label="停止">
        ⏹
      </button>
      <button onclick={nextBar} disabled={!transport.available} title="次の小節頭へ(→)" aria-label="次の小節頭へ">⏩</button>
      <button onclick={seekEnd} disabled={!transport.available} title="終端へ(End)" aria-label="終端へ">⏭</button>
      <button
        class:loop-on={loopOn}
        aria-label="ループ再生"
        aria-pressed={loopOn}
        onclick={toggleLoop}
        disabled={!transport.available}
        title="ループ再生(L)。ルーラーで範囲選択するとその区間、なければ曲全体"
      >
        🔁
      </button>
      <button
        class:loop-on={transport.metronome}
        aria-label="メトロノーム"
        aria-pressed={!!transport.metronome}
        onclick={toggleMetronome}
        disabled={!transport.available}
        title="メトロノーム(拍ごとにクリック。小節頭は高い音)"
      >
        ⏱
      </button>
      <button
        class="rec"
        aria-label={transport.recording ? "録音を止める" : "録音"}
        class:rec-on={transport.recording}
        onclick={toggleRecord}
        disabled={!transport.available}
        title={transport.midi_recording
          ? "MIDI 録音を止めてクリップとして配置"
          : transport.recording
            ? "録音を止めて音声トラックにクリップとして配置"
            : midiArm
              ? `MIDI 録音(🎹 ${midiArm.name})。再生ヘッド位置から録り、停止するとそのトラックに置かれます`
              : "録音(既定の入力デバイス)。再生ヘッド位置から録り、停止すると音声トラックに置かれます"}
      >
        {midiArm && !transport.recording ? "⏺🎹" : "⏺"}
      </button>
      {#if transport.playing || dspWarn}
        <span
          class="dsp"
          class:dsp-warn={dspWarn}
          title={`オーディオ処理の負荷(この再生の開始から)\n平均 ${dspAvg.toFixed(0)}% / 直近 3 秒の最大 ${dspMax.toFixed(0)}%\n処理落ち(Glaux の計算が間に合わない): ${dspCounts.overruns} 回\n呼び出し遅延(他の処理に CPU を奪われた): ${dspCounts.late} 回\n再生データの差し替え(編集で音が切り直される): ${dspCounts.swaps} 回`}
        >
          DSP {dspAvg.toFixed(0)}%{#if dspCounts.overruns + dspCounts.late > 0}
            ⚠{dspCounts.overruns}/{dspCounts.late}{/if}
        </span>
      {/if}
      {#if transport.recording && !transport.midi_recording}
        <span
          class="rec-meter"
          title={`入力レベル ${(recLevel).toFixed(0)} dBFS(目安: -12〜-6dB)`}
        >
          <span class="rec-meter-fill" class:hot={recLevel > -3} style="width:{Math.max(0, Math.min(100, ((recLevel + 60) / 60) * 100))}%"></span>
        </span>
      {/if}
      {#if recordNotice}
        <span class="rec-notice" title={recordNotice}>{recordNotice}</span>
      {/if}
      {#if lastRecorded}
        <button class="rec-midi" onclick={transcribeLast} title="録音(鼻歌・歌など単旋律)を譜起こしして MIDI クリップにする">
          ♪ MIDI 化
        </button>
      {/if}
      {#if editingBpm}
        <!-- svelte-ignore a11y_autofocus -->
        <input
          class="bpm-input"
          type="number"
          min="20"
          max="300"
          step="0.5"
          autofocus
          bind:value={bpmInput}
          onkeydown={onBpmKeydown}
          onblur={commitBpm}
        />
      {:else}
        {#if harmonyStore.view?.key}
          <span
            class="stat key-stat"
            title={`キー(ノートからの推定。確からしさ ${Math.round(harmonyStore.view.key.confidence * 100)}%)。ルーラーに小節ごとのコード`}
            >{harmonyStore.view.key.name}</span
          >
        {/if}
        <button class="stat bpm-btn" onclick={startBpmEdit} title="クリックで BPM を編集">
          {bpm} BPM
        </button>
      {/if}
      {#if editingSig}
        <span class="sig-edit">
          <!-- svelte-ignore a11y_autofocus -->
          <input
            class="sig-input"
            type="number"
            min="1"
            max="32"
            autofocus
            bind:value={sigNumInput}
            onkeydown={onSigKeydown}
            onblur={(e) => {
              // 分母セレクトへの移動では確定しない
              const to = e.relatedTarget as HTMLElement | null;
              if (!to || !to.classList.contains("sig-den")) commitSig();
            }}
          />
          /
          <select
            class="sig-den"
            bind:value={sigDenInput}
            onkeydown={onSigKeydown}
            onchange={commitSig}
            onblur={commitSig}
          >
            {#each [2, 4, 8, 16] as d (d)}
              <option value={d}>{d}</option>
            {/each}
          </select>
        </span>
      {:else}
        <button
          class="stat bpm-btn"
          onclick={startSigEdit}
          title={hasSigChanges
            ? "クリックで先頭の拍子を編集(曲中に拍子変更あり。変更はルーラーに表示)"
            : "クリックで拍子を編集(曲中での変更は AI に「◯小節目から 7/8 にして」と頼めます)"}
        >
          {timeSig}{#if hasSigChanges}*{/if}
        </button>
      {/if}
      <span class="stat ver-stat" title="版数(編集・取り消し・やり直しのたびに増える)">v{projectVersion}</span>
      <button
        onclick={doUndo}
        disabled={entries.length === 0}
        title={entries.length > 0
          ? `取り消す: ${entries[entries.length - 1].label}(Ctrl+Z)`
          : "取り消せる編集はありません"}
        aria-label="取り消し">↶<span class="btn-label"> 取り消し</span></button
      >
      <button
        onclick={doRedo}
        disabled={redoable.length === 0}
        title={redoable.length > 0
          ? `やり直す: ${redoable[0].label}(Ctrl+Shift+Z / Ctrl+Y)`
          : "やり直せる編集はありません"}
        aria-label="やり直し">↷<span class="btn-label"> やり直し</span></button
      >
      <div class="master" title={`マスター音量 ${(project?.master.volume_db ?? 0).toFixed(1)} dB`}>
        <span class="master-icon">🔊</span>
        <input
          type="range"
          min="-40"
          max="6"
          step="0.5"
          value={project?.master.volume_db ?? 0}
          onchange={setMasterVolume}
        />
        <span class="stat master-db">{(project?.master.volume_db ?? 0).toFixed(1)} dB</span>
        <button
          class="master-fx"
          class:on={soundDesignStore.focus?.trackId === MASTER_FOCUS_ID}
          onclick={() =>
            (soundDesignStore.focus =
              soundDesignStore.focus?.trackId === MASTER_FOCUS_ID
                ? null
                : { trackId: MASTER_FOCUS_ID, trackName: "マスター" })}
          title="マスターのエフェクト(曲全体に掛かるコンプ・EQ・リバーブ等)"
        >
          🎛{#if (project?.master.effects.length ?? 0) > 0}<span class="fx-count">{project?.master.effects.length}</span>{/if}
        </button>
      </div>
      <button
        onclick={doExport}
        disabled={exporting}
        title="WAV に書き出す(プロジェクト内 export フォルダ)"
        aria-label="WAV に書き出す"
      >
        {#if exporting}書き出し中…{:else}⬇<span class="export-label"> WAV</span>{/if}
      </button>
      <button onclick={() => (showSettings = true)} title="設定" aria-label="設定">⚙</button>
    </div>
    {#if indicator !== "idle"}
      <div class="ai-indicator" class:thinking={indicator === "session"}>
        <span class="pulse"></span>
        {#if indicator === "calling"}
          AI が{toolDoing(aiTool)}…
        {:else}
          AI が作業中です…
        {/if}
      </div>
    {/if}
    <div class="mcp">
      {#if info}
        <button class="mcp-url" onclick={copyMcpUrl} title="Claude Code への登録コマンドをコピー">
          {mcpCopied ? "コピーしました ✓" : `MCP: ${info.mcp_url}`}
        </button>
      {/if}
    </div>
  </header>

  {#if error}
    <div class="error">{error}</div>
  {/if}
  {#if saveError}
    <div class="error save-error" role="alert">
      保存できませんでした。編集は画面上には残っていますが、このまま閉じると失われます。
      ほかのアプリがファイルを使っていないか(OneDrive の同期など)確かめてください。次の編集で保存し直します。
      <br /><small>{saveError}</small>
    </div>
  {/if}

  <main>
    <section class="timeline-area">
      {#if project}
        <!-- スクローラーとピアノロールのオーバーレイは親を分ける:
             overflow 要素の absolute 子はスクロール原点に張り付くため、
             スクロール中に開くと画面外に出てしまう -->
        <div class="timeline-scroll">
          <Timeline
            {project}
            playheadTick={transport.tick}
            playing={transport.playing}
            onSeek={seek}
          />
        </div>
        {#if pianoRollStore.focus}
          <!-- 分割時は上下 2 ペイン。各 PianoRoll のルート(.overlay)は
               absolute inset:0 なので、pane で相対配置の枠を与える -->
          <div class="roll-split" class:two={pianoRollStore.second !== null}>
            <div class="roll-pane">
              <PianoRoll
                {project}
                pane="main"
                playheadTick={transport.tick}
                playing={transport.playing}
                onSeek={seek}
              />
            </div>
            {#if pianoRollStore.second}
              <div class="roll-pane second">
                <PianoRoll
                  {project}
                  pane="second"
                  playheadTick={transport.tick}
                  playing={transport.playing}
                  onSeek={seek}
                />
              </div>
            {/if}
          </div>
        {/if}
        <SoundDesignPanel {project} />
      {:else}
        <div class="loading">読み込み中…</div>
      {/if}
    </section>
    <div class="row-handle" onpointerdown={startRowResize} title="ドラッグで高さを調整">
      <button
        class="collapse-btn"
        onpointerdown={(e) => e.stopPropagation()}
        onclick={toggleBottom}
        title={bottomCollapsed ? "チャットと履歴を表示する" : "チャットと履歴を隠して、タイムライン・ピアノロールを広く使う"}
        aria-label={bottomCollapsed ? "下のパネルを表示" : "下のパネルを隠す"}
        aria-expanded={!bottomCollapsed}
      >{bottomCollapsed ? "▴ チャット・履歴" : "▾"}</button>
    </div>
    <!-- 隠しても部品は残す(チャットの表示中の会話が消えないように) -->
    <section class="bottom-area" class:collapsed={bottomCollapsed} style="height:{bottomHeight}px">
      <div class="chat-section" style="flex:0 0 {chatFrac * 100}%">
        <ChatPanel />
      </div>
      <div class="col-handle" onpointerdown={startColResize} title="ドラッグで幅を調整"></div>
      <div class="history-section">
        <HistoryPanel {entries} total={historyTotal} {redoable} />
      </div>
    </section>
  </main>

  {#if showSettings}
    <SettingsPanel onClose={closeSettings} />
  {/if}

  <footer>
    {#if exportMsg}
      <code class="export-msg">{exportMsg}</code>
    {:else if info}
      <code title="プロジェクトフォルダ">{info.project_dir}</code>
    {/if}
    {#if audioDev}
      <button
        class="audio-dev"
        onclick={() => {
          showSettings = true;
        }}
        title="使用中のオーディオデバイス(クリックで設定を開いて変更)"
      >
        🔈 {audioDev.output ?? "なし"}{audioDev.rate ? ` ${(audioDev.rate / 1000).toFixed(1)}kHz` : ""} · 🎤
        {audioDev.input ?? "なし"}{audioDev.midi ? ` · 🎹 ${audioDev.midi}` : ""}
      </button>
    {/if}
  </footer>
</div>

<Toasts />

<style>
  .layout {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  /* ヘッダーは常に 1 行(ボタンが増えても縦に伸ばさない)。
     幅が足りないときは折り返さずに横スクロールする */
  header {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 8px 14px;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
    white-space: nowrap;
    overflow-x: auto;
    overflow-y: hidden;
    scrollbar-width: thin;
  }

  header button:not(.mcp-url) {
    white-space: nowrap;
    flex-shrink: 0;
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 8px;
    font-weight: 600;
    font-size: 15px;
  }

  /* ブランドガイド: アプリ内では白フチの全身版を使い、影・発光は加えない */
  .owl {
    display: block;
    width: 28px;
    height: 28px;
    flex-shrink: 0;
  }

  .transport {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .play {
    min-width: 44px;
    font-size: 14px;
  }

  .roll-split {
    position: absolute;
    inset: 0;
    z-index: 5;
    display: flex;
    flex-direction: column;
  }

  .roll-pane {
    position: relative;
    flex: 1;
    min-height: 0;
  }

  .roll-pane.second {
    border-top: 2px solid var(--accent-dim);
  }

  .loop-on {
    border-color: var(--accent-dim);
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 14%, var(--bg-panel));
  }

  .rec {
    color: var(--danger);
  }

  .rec-on {
    border-color: var(--danger);
    background: color-mix(in srgb, var(--danger) 25%, var(--bg-panel));
    animation: rec-blink 1s ease-in-out infinite;
  }

  @keyframes rec-blink {
    50% {
      background: color-mix(in srgb, var(--danger) 55%, var(--bg-panel));
    }
  }

  /* ヘッダーの高さを変えないよう 1 行に収め、はみ出しは省略(全文はホバーで) */
  .rec-notice {
    font-size: 11px;
    color: var(--text-dim);
    margin-left: 4px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 16em;
    min-width: 0;
  }

  .dsp {
    font-size: 11px;
    color: var(--text-dim);
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }

  .dsp-warn {
    color: var(--warn);
  }

  .rec-midi {
    border-color: var(--accent-dim);
    color: var(--accent);
    white-space: nowrap;
    flex-shrink: 0;
  }

  .bpm-btn {
    border: none;
    cursor: pointer;
  }

  .bpm-btn:hover {
    color: var(--accent);
  }

  .bpm-input {
    width: 70px;
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--accent-dim);
    border-radius: 4px;
    padding: 2px 6px;
    font-size: 13px;
  }

  .sig-edit {
    display: flex;
    align-items: center;
    gap: 3px;
    color: var(--text-dim);
  }

  .sig-input {
    width: 44px;
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--accent-dim);
    border-radius: 4px;
    padding: 2px 6px;
    font-size: 13px;
  }

  .sig-den {
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--accent-dim);
    border-radius: 4px;
    padding: 2px 4px;
    font-size: 13px;
  }

  .master {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-left: 8px;
  }

  .master-icon {
    font-size: 13px;
  }

  .master input[type="range"] {
    width: 110px;
    accent-color: var(--accent);
  }

  .stat {
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
    padding: 2px 8px;
    background: var(--bg);
    border-radius: 4px;
    font-size: 13px;
  }

  .ai-indicator {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 3px 12px;
    border-radius: 12px;
    background: color-mix(in srgb, var(--ai) 18%, transparent);
    color: var(--ai);
    font-size: 12px;
    white-space: nowrap;
  }

  /* 考え中(呼び出しの合間)は控えめに、ゆっくり点滅 */
  .ai-indicator.thinking {
    opacity: 0.65;
  }

  .ai-indicator.thinking .pulse {
    animation-duration: 2.2s;
  }

  .pulse {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--ai);
    animation: pulse 1s ease-in-out infinite;
  }

  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
      transform: scale(1);
    }
    50% {
      opacity: 0.35;
      transform: scale(0.7);
    }
  }

  /* 幅が足りないときは MCP の URL 表示から縮める(操作ボタンを優先) */
  .mcp {
    margin-left: auto;
    min-width: 60px;
    flex: 0 1 auto;
    overflow: hidden;
  }

  .mcp-url {
    font-family: Consolas, monospace;
    font-size: 12px;
    color: var(--text-dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 360px;
    width: 100%;
    flex-shrink: 1;
  }

  main {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }

  .timeline-area {
    flex: 1;
    min-height: 0;
    overflow: hidden;
    position: relative;
    display: flex;
    flex-direction: column;
  }

  /* 狭い画面(ノート PC の 150% 表示など)では、ヘッダーの文字を減らして右端の ⚙ まで収める */
  @media (max-width: 1320px) {
    .btn-label,
    .ver-stat,
    .key-stat {
      display: none;
    }
    .master input[type="range"] {
      width: 70px;
    }
  }

  @media (max-width: 1120px) {
    .master-db,
    .export-label {
      display: none;
    }
    .master input[type="range"] {
      width: 56px;
    }
  }

  .timeline-scroll {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }

  .bottom-area {
    flex-shrink: 0;
    background: var(--bg-panel);
    display: flex;
    min-height: 0;
    /* 低い画面でタイムラインが潰れないように(高さ 330px の画面で 0px になっていた) */
    max-height: 45vh;
  }

  .bottom-area.collapsed {
    display: none;
  }

  .row-handle {
    position: relative;
  }

  .collapse-btn {
    position: absolute;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    z-index: 2;
    padding: 0 10px;
    font-size: 10px;
    line-height: 14px;
    border-radius: 7px;
    cursor: pointer;
  }

  .row-handle {
    height: 5px;
    flex-shrink: 0;
    cursor: row-resize;
    background: var(--border);
    touch-action: none;
  }

  .row-handle:hover,
  .col-handle:hover {
    background: var(--accent-dim);
  }

  .col-handle {
    width: 5px;
    flex-shrink: 0;
    cursor: col-resize;
    background: var(--border);
    touch-action: none;
  }

  .chat-section {
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  .history-section {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
  }

  footer {
    padding: 4px 14px;
    background: var(--bg-panel);
    border-top: 1px solid var(--border);
    color: var(--text-dim);
    flex-shrink: 0;
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
  }

  .master-fx {
    padding: 2px 6px;
  }

  .master-fx.on {
    border-color: var(--accent-dim);
    color: var(--accent);
  }

  .fx-count {
    font-size: 10px;
    margin-left: 2px;
    color: var(--accent);
  }

  .audio-dev {
    border: none;
    background: none;
    padding: 0;
    font-size: 11px;
    color: var(--text-dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 55%;
    cursor: pointer;
  }

  .audio-dev:hover {
    color: var(--accent);
  }

  .rec-meter {
    display: inline-block;
    position: relative;
    width: 60px;
    height: 8px;
    border-radius: 2px;
    background: var(--bg);
    border: 1px solid var(--border);
    overflow: hidden;
    flex-shrink: 0;
  }

  .rec-meter-fill {
    position: absolute;
    inset: 0 auto 0 0;
    background: var(--ok);
  }

  .rec-meter-fill.hot {
    background: var(--danger);
  }

  .export-msg {
    color: var(--accent);
  }

  .error {
    background: var(--danger-bg);
    color: var(--danger-text);
    padding: 6px 14px;
  }

  .save-error {
    font-weight: 600;
  }

  .loading {
    padding: 40px;
    color: var(--text-dim);
  }
</style>
