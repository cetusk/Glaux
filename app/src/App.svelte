<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "./lib/api";
  import { shouldYieldKey } from "./lib/keys";
  import { toolDoing } from "./lib/toolLabels";
  import Toasts from "./lib/Toasts.svelte";
  import Icon from "./lib/Icon.svelte";
  import { showToast } from "./lib/toast.svelte";
  import { harmonyStore, refreshHarmony } from "./lib/harmony.svelte";
  import {
    barAtTick,
    buildBars,
    formatPosition,
    formatSeconds,
    nextBarHead,
    prevBarHead,
    tickToSeconds,
  } from "./lib/barMap";
  import type { AppInfo, EntrySummary, Project } from "./lib/types";
  import { pollTransport, startTransportPolling, transportStore } from "./lib/transport.svelte";
  import Timeline from "./lib/Timeline.svelte";
  import HistoryPanel from "./lib/HistoryPanel.svelte";
  import ChatPanel from "./lib/ChatPanel.svelte";
  import ProjectMenu from "./lib/ProjectMenu.svelte";
  import PianoRoll from "./lib/PianoRoll.svelte";
  import SettingsPanel from "./lib/SettingsPanel.svelte";
  import ExportDialog from "./lib/ExportDialog.svelte";
  import WelcomeDialog from "./lib/WelcomeDialog.svelte";
  import SoundDesignPanel from "./lib/SoundDesignPanel.svelte";
  import InstrumentPicker from "./lib/InstrumentPicker.svelte";
  import Mixer from "./lib/Mixer.svelte";
  import { applyTheme, openSettings, settings, settingsUi, welcomeUi } from "./lib/settings.svelte";
  import { chatStatus } from "./lib/aiStatus.svelte";
  import {
    inspectorStore,
    midiArmStore,
    pianoRollStore,
    selectionStore,
    soundDesignStore,
    viewStore,
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
  // 設定画面の開閉(開くページも持つ。settings.svelte.ts の settingsUi)
  let showExport = $state(false);

  function closeSettings() {
    settingsUi.open = false;
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


  /// 取り直しの通し番号。後から始めた取り直しがあれば、先に始めた方の結果は捨てる
  /// (トラックだけの取り直しと全体の取り直しが追い越し合って、古い内容で上書きしないように)
  let syncSeq = 0;

  async function refresh() {
    const seq = ++syncSeq;
    try {
      const [p, h] = await Promise.all([api.getProject(), api.getHistory()]);
      if (seq !== syncSeq) return;
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

  // ---- 変わったトラックだけの取り直し ----
  // 変更がトラックの中だけ(クリップ・ノート・つまみ・エフェクト・オートメーション・トラックの設定)で、
  // 版が手元の続きになっているときは、そのトラックだけを取り直して差し替える。
  // それ以外(トラックの追加・削除・並べ替え、テンポ、マスター、切り替えなど)は全体を取り直す

  /// 合流の窓の間にたまった変更通知
  let pendingChanges: api.ProjectChangedEvent[] = [];

  /** トラックだけで済むなら、そのトラックの ID と、取り直した後の版。済まなければ null */
  function changedTracks(evs: api.ProjectChangedEvent[]): { ids: Set<string>; version: number } | null {
    const p = project;
    if (!p || evs.length === 0) return null;
    const ids = new Set<string>();
    let v = projectVersion;
    for (const ev of evs) {
      if (ev.project_version !== v + 1) return null;
      v = ev.project_version;
      if (!ev.changes || ev.changes.length === 0) return null;
      for (const c of ev.changes) {
        let id: string | undefined;
        switch (c.kind) {
          case "track_prop_changed":
            id = c.id;
            break;
          case "clips_changed":
          case "param_changed":
          case "device_changed":
          case "effects_changed":
          case "automation_changed":
            id = c.track;
            break;
          case "notes_changed":
            id = p.tracks.find((t) => t.clips.some((cl) => cl.id === c.clip))?.id;
            break;
          default:
            return null;
        }
        if (!id || !p.tracks.some((t) => t.id === id)) return null;
        ids.add(id);
      }
    }
    return { ids, version: v };
  }

  async function sync() {
    const evs = pendingChanges;
    pendingChanges = [];
    const partial = changedTracks(evs);
    if (!partial) return refresh();
    const seq = ++syncSeq;
    try {
      const [t, h] = await Promise.all([api.getTracks([...partial.ids]), api.getHistory()]);
      if (seq !== syncSeq) return;
      const p = project;
      if (!p || t.project_version !== partial.version || t.tracks.length !== partial.ids.size) return refresh();
      for (const nt of t.tracks) {
        const i = p.tracks.findIndex((x) => x.id === nt.id);
        if (i < 0) return refresh();
        p.tracks[i] = nt;
      }
      projectVersion = t.project_version;
      entries = h.entries;
      historyTotal = h.total;
      redoable = h.redoable ?? [];
      refreshHarmony();
      error = null;
    } catch {
      refresh();
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
    sync();
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
      if (settings.bufferFrames) {
        await api.setBufferSize(settings.bufferFrames).catch(() => {});
      }
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
          showToast(
            "warn",
            `前回の MIDI 入力「${settings.midiInput}」が見つかりません(接続して設定で選び直してください)`,
          );
        });
      }
      refreshAudioDev();
    })();

    let idleTimer: ReturnType<typeof setTimeout> | undefined;
    const unlistenChanged = api.onProjectChanged((ev) => {
      // 保存の失敗は、次に保存が成功するまで出し続ける(再取得で消える error とは分ける)
      saveError = ev?.save_error ?? null;
      pendingChanges.push(ev);
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
      if (e.key === "Escape" && settingsUi.open) {
        e.preventDefault();
        closeSettings();
        return;
      }
      if (e.key === "Escape" && showExport) {
        e.preventDefault();
        showExport = false;
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
  let dspBase: { overruns: number; late: number; swaps: number; xruns: number } | null = null;
  let dspCounts = $state({ overruns: 0, late: 0, swaps: 0, xruns: 0 });
  $effect(() => {
    const d = transport.dsp;
    if (!d) return;
    if (!transport.playing) {
      dspBase = null;
      return;
    }
    if (!dspBase) dspBase = { overruns: d.overruns, late: d.late, swaps: d.swaps, xruns: d.xruns ?? 0 };
    dspAvg = d.avg_pct;
    dspMaxWindow = [...dspMaxWindow.slice(-29), d.max_pct];
    dspMax = Math.max(...dspMaxWindow);
    dspCounts = {
      overruns: d.overruns - dspBase.overruns,
      late: d.late - dspBase.late,
      swaps: d.swaps - dspBase.swaps,
      xruns: (d.xruns ?? 0) - dspBase.xruns,
    };
  });
  const dspDrops = $derived(dspCounts.overruns + dspCounts.late + dspCounts.xruns);
  const dspWarn = $derived(dspDrops > 0 || dspMax > 80);

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

  // 録音の結果はトーストで知らせる(以前はヘッダーの通知欄と「♪ MIDI 化」ボタン)
  async function finishMidiRecording() {
    const r = await api.midiRecordStop(midiArmStore.trackId, settings.midiQuantize);
    await pollTransport();
    showToast("ok", `MIDI 録音を配置しました: ${r.notes} ノート`);
  }

  async function finishRecording() {
    const r = await api.recordStop(null, settings.autoGain);
    await pollTransport();
    const warn =
      r.clipped > 0 ? `(${r.clipped} サンプルがクリップしました。入力レベルを下げてください)` : "";
    const gain = r.gain_db > 0.5 ? `、音量 +${r.gain_db.toFixed(0)}dB` : "";
    const target = { clipId: r.clip_id, trackId: r.track_id };
    showToast(r.clipped > 0 ? "warn" : "ok", `録音を配置しました: ${r.seconds.toFixed(1)} 秒${gain}${warn}`, {
      action: { label: "MIDI にする", run: () => transcribeRecorded(target) },
      ms: 20000,
    });
  }

  /// 録音した鼻歌をそのまま MIDI にしてピアノロールで開く
  async function transcribeRecorded(target: { clipId: string; trackId: string }) {
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
      showToast("ok", `${r.note_count} ノートを MIDI にしました。AI に「キーを確認して整えて」と頼めます`);
    } catch (e) {
      error = String(e);
    }
  }

  /** 聴き方を切り替えている(モノ・サイド・入替・クロスフィード)。忘れないよう再生ボタンの横に出す */
  const monitorLabel = $derived.by(() => {
    const m = transport.monitor;
    if (!m) return null;
    const mode = { stereo: "", mono: "モノ", side: "サイド", swap: "左右入替" }[m.mode] ?? "";
    const spk = { off: "", phone: "スマホ", laptop: "ノート PC", front: "仮想スピーカー" }[m.speaker ?? "off"] ?? "";
    const parts = [mode, m.crossfeed ? "クロスフィード" : "", spk].filter(Boolean);
    return parts.length ? parts.join("+") : null;
  });
  async function resetMonitor() {
    try {
      await api.transportSetMonitor("stereo", false);
      await api.transportSetSpeaker("off");
      await pollTransport();
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
          showToast("ok", `カウントイン ${settings.countInBars} 小節のあと録音位置になります`);
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

  // ---- 表示窓(位置・時間) ----
  const posBars = $derived.by(() => {
    if (!project) return [];
    return buildBars(project, Math.max(contentEndTick, transport.tick) + 1, 1, 2);
  });
  const posText = $derived(project ? formatPosition(posBars, transport.tick, project.ppq) : "1.1.1");
  const timeText = $derived(
    project ? formatSeconds(tickToSeconds(project.tempo_map, transport.tick, project.ppq)) : "0:00.0",
  );

  const bpm = $derived(project?.tempo_map[0]?.bpm ?? 120);
  const timeSig = $derived(
    project ? `${project.time_sig_map[0]?.num ?? 4}/${project.time_sig_map[0]?.den ?? 4}` : "4/4",
  );
  const hasSigChanges = $derived((project?.time_sig_map.length ?? 0) > 1);
</script>

<div class="layout">
  <!-- ヘッダー: 左 = 曲 / 中央 = 再生と表示窓 / 右 = 取り消し・書き出し・設定
       (以前は 1 行にすべてを並べて横にはみ出していた。負荷・デバイス・MCP はステータスバーへ、
       マスターの音量とエフェクトはタイムラインのマスター行へ) -->
  <header>
    <div class="h-left">
      <img class="owl" src="/glaux-icon.png" alt="Glaux" width="28" height="28" />
      <ProjectMenu title={project?.meta.title ?? "…"} />
      <div class="view-switch" role="tablist" aria-label="表示の切り替え">
        <button
          role="tab"
          aria-selected={viewStore.main === "timeline"}
          class:on={viewStore.main === "timeline"}
          onclick={() => (viewStore.main = "timeline")}
          title="タイムライン(曲の流れ・クリップ・ピアノロール)"><Icon name="rows-2" />タイムライン</button
        >
        <button
          role="tab"
          aria-selected={viewStore.main === "mixer"}
          class:on={viewStore.main === "mixer"}
          onclick={() => (viewStore.main = "mixer")}
          title="ミキサー(音量・パン・送り・エフェクトのつなぎ方)"><Icon name="sliders-horizontal" />ミキサー</button
        >
      </div>
    </div>
    <div class="h-center">
      <div class="tp">
        <button class="btn icon" onclick={seekStart} disabled={!transport.available} title="先頭へ(Home)" aria-label="先頭へ"
          ><Icon name="skip-back" /></button
        >
        <button class="btn icon" onclick={prevBar} disabled={!transport.available} title="前の小節へ(←)" aria-label="前の小節へ"
          ><Icon name="rewind" /></button
        >
        <button
          class="btn icon play"
          aria-label={transport.playing ? "一時停止" : "再生"}
          onclick={togglePlay}
          disabled={!transport.available}
          title={transport.available ? "再生 / 一時停止(Space)" : "オーディオデバイスが利用できません"}
          ><Icon name={transport.playing ? "pause" : "play"} /></button
        >
        <button class="btn icon" onclick={stopPlayback} disabled={!transport.available} title="停止" aria-label="停止"
          ><Icon name="square" /></button
        >
        <button class="btn icon" onclick={nextBar} disabled={!transport.available} title="次の小節へ(→)" aria-label="次の小節へ"
          ><Icon name="fast-forward" /></button
        >
        <button class="btn icon" onclick={seekEnd} disabled={!transport.available} title="終端へ(End)" aria-label="終端へ"
          ><Icon name="skip-forward" /></button
        >
      </div>
      <div class="tp">
        <button
          class="btn icon"
          class:on={loopOn}
          aria-label="ループ再生"
          aria-pressed={loopOn}
          onclick={toggleLoop}
          disabled={!transport.available}
          title="ループ再生(L)。ルーラーで範囲を選ぶとその区間、なければ曲全体"
          ><Icon name="repeat" /></button
        >
        <button
          class="btn icon"
          class:on={transport.metronome}
          aria-label="メトロノーム"
          aria-pressed={!!transport.metronome}
          onclick={toggleMetronome}
          disabled={!transport.available}
          title="メトロノーム(拍ごとにクリック。小節頭は高い音)"
          ><Icon name="metronome" /></button
        >
        {#if monitorLabel}
          <button
            class="btn icon monitor-on"
            aria-label={`聴き方: ${monitorLabel}(押すとステレオに戻す)`}
            onclick={resetMonitor}
            title={`聴き方を「${monitorLabel}」に切り替えています(ミキサーのモニター列)。押すとふつうのステレオに戻します。書き出しには入りません`}
            ><Icon name="headphones" /></button
          >
        {/if}
        <button
          class="btn icon rec"
          aria-label={transport.recording ? "録音を止める" : "録音"}
          class:rec-on={transport.recording}
          onclick={toggleRecord}
          disabled={!transport.available}
          title={transport.midi_recording
            ? "MIDI 録音を止めてクリップとして配置"
            : transport.recording
              ? "録音を止めて音声トラックにクリップとして配置"
              : midiArm
                ? `MIDI 録音(${midiArm.name})。再生ヘッドの位置から録り、止めるとそのトラックに置かれます`
                : "録音(既定の入力デバイス)。再生ヘッドの位置から録り、止めると音声トラックに置かれます"}
          ><Icon name="circle" fill /></button
        >
      </div>
      <!-- 表示窓: 押せるのはテンポと拍子だけ(▾ の付いた欄) -->
      <div class="lcd" class:recording={transport.recording}>
        <div class="f pos" title="位置(小節.拍.16 分)">
          <span class="lbl">位置</span><span class="val">{posText}</span>
        </div>
        <div class="f" title="時間(分:秒)">
          <span class="lbl">時間</span><span class="val">{timeText}</span>
        </div>
        {#if transport.recording && !transport.midi_recording}
          <div class="f" title={`入力レベル ${recLevel.toFixed(0)} dBFS(目安: -12〜-6dB)`}>
            <span class="lbl rec-lbl">録音中 · 入力</span>
            <span class="rec-meter"
              ><span
                class="rec-meter-fill"
                class:hot={recLevel > -3}
                style="width:{Math.max(0, Math.min(100, ((recLevel + 60) / 60) * 100))}%"
              ></span></span
            >
          </div>
        {:else if transport.midi_recording}
          <div class="f"><span class="lbl rec-lbl">MIDI 録音中</span><span class="val">{midiArm?.name ?? ""}</span></div>
        {/if}
        {#if editingBpm}
          <div class="f">
            <span class="lbl">テンポ</span>
            <!-- svelte-ignore a11y_autofocus -->
            <input
              class="lcd-input"
              type="number"
              min="20"
              max="300"
              step="0.5"
              autofocus
              bind:value={bpmInput}
              onkeydown={onBpmKeydown}
              onblur={commitBpm}
            />
          </div>
        {:else}
          <button
            class="f edit"
            onclick={startBpmEdit}
            title={(project?.tempo_map.length ?? 0) > 1
              ? "クリックで先頭のテンポ(BPM)を編集(曲の途中にテンポの変更あり。途中の変更はルーラーの右クリックで)"
              : "クリックでテンポ(BPM)を編集(曲の途中から変えるときはルーラーを右クリック)"}
          >
            <span class="lbl">テンポ</span><span class="val">{bpm}{#if (project?.tempo_map.length ?? 0) > 1}*{/if}</span>
          </button>
        {/if}
        {#if editingSig}
          <div class="f">
            <span class="lbl">拍子</span>
            <span class="sig-edit">
              <!-- svelte-ignore a11y_autofocus -->
              <input
                class="lcd-input sig-input"
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
              />/<select class="sig-den" bind:value={sigDenInput} onkeydown={onSigKeydown} onchange={commitSig} onblur={commitSig}>
                {#each [2, 4, 8, 16] as d (d)}
                  <option value={d}>{d}</option>
                {/each}
              </select>
            </span>
          </div>
        {:else}
          <button
            class="f edit"
            onclick={startSigEdit}
            title={hasSigChanges
              ? "クリックで先頭の拍子を編集(曲の途中に拍子の変更あり。途中の変更はルーラーの右クリックで)"
              : "クリックで拍子を編集(曲の途中から変えるときはルーラーを右クリック)"}
          >
            <span class="lbl">拍子</span><span class="val">{timeSig}{#if hasSigChanges}*{/if}</span>
          </button>
        {/if}
        {#if harmonyStore.view?.key}
          <div
            class="f key"
            title={`キー(ノートからの推定。確からしさ ${Math.round(harmonyStore.view.key.confidence * 100)}%)。ルーラーに小節ごとのコード`}
          >
            <span class="lbl">キー(推定)</span><span class="val">{harmonyStore.view.key.name}</span>
          </div>
        {/if}
      </div>
    </div>
    <div class="h-right">
      {#if indicator !== "idle"}
        <div class="ai-indicator" class:thinking={indicator === "session"}>
          <Icon name="sparkles" size={14} />
          <span class="ai-text">{indicator === "calling" ? `AI が${toolDoing(aiTool)}…` : "AI が作業中です…"}</span>
        </div>
      {/if}
      <button
        class="btn icon"
        onclick={doUndo}
        disabled={entries.length === 0}
        title={entries.length > 0 ? `取り消す: ${entries[entries.length - 1].label}(Ctrl+Z)` : "取り消せる編集はありません"}
        aria-label="取り消し"><Icon name="undo-2" /></button
      >
      <button
        class="btn icon"
        onclick={doRedo}
        disabled={redoable.length === 0}
        title={redoable.length > 0 ? `やり直す: ${redoable[0].label}(Ctrl+Shift+Z / Ctrl+Y)` : "やり直せる編集はありません"}
        aria-label="やり直し"><Icon name="redo-2" /></button
      >
      <span class="sep"></span>
      <button
        class="btn"
        onclick={() => (showExport = true)}
        title="書き出し(WAV・トラックごと・MIDI。形式・範囲・音量を選べる)"
        aria-label="書き出し"><Icon name="download" /><span class="export-label">書き出し</span></button
      >
      <button class="btn icon" onclick={() => openSettings()} title="設定" aria-label="設定"><Icon name="settings" /></button>
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
        <!-- インスペクターを開いている間は、その幅だけ押し縮める(上に被せない) -->
        {#if viewStore.main === "mixer"}
          <div class="mixer-host" style={soundDesignStore.focus ? `margin-right:${inspectorStore.width}px` : ""}>
            <Mixer {project} />
          </div>
        {:else}
        <div class="timeline-scroll" style={soundDesignStore.focus ? `margin-right:${inspectorStore.width}px` : ""}>
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
          <div
            class="roll-split"
            class:two={pianoRollStore.second !== null}
            style={soundDesignStore.focus ? `right:${inspectorStore.width}px` : ""}
          >
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
        {/if}
        <SoundDesignPanel {project} />
        <InstrumentPicker {project} />
      {:else}
        <div class="loading">読み込み中…</div>
      {/if}
    </section>
    <div
      class="row-handle"
      role="separator"
      aria-orientation="horizontal"
      class:collapsed={bottomCollapsed}
      onpointerdown={(e) => !bottomCollapsed && startRowResize(e)}
      title={bottomCollapsed ? undefined : "ドラッグで高さを調整"}
    >
      <button
        class="btn collapse-btn"
        onpointerdown={(e) => e.stopPropagation()}
        onclick={toggleBottom}
        title={bottomCollapsed ? "チャットと履歴を表示する" : "チャットと履歴を隠して、タイムライン・ピアノロールを広く使う"}
        aria-label={bottomCollapsed ? "下のパネルを表示" : "下のパネルを隠す"}
        aria-expanded={!bottomCollapsed}
      ><Icon name={bottomCollapsed ? "chevron-up" : "chevron-down"} />{#if bottomCollapsed}チャット・履歴{/if}</button>
    </div>
    <!-- 隠しても部品は残す(チャットの表示中の会話が消えないように) -->
    <section class="bottom-area" class:collapsed={bottomCollapsed} style="height:{bottomHeight}px">
      <div class="chat-section" style="flex:0 0 {chatFrac * 100}%">
        <ChatPanel />
      </div>
      <div class="col-handle" role="separator" aria-orientation="vertical" onpointerdown={startColResize} title="ドラッグで幅を調整"></div>
      <div class="history-section">
        <HistoryPanel {entries} total={historyTotal} {redoable} />
      </div>
    </section>
  </main>

  {#if settingsUi.open}
    <SettingsPanel onClose={closeSettings} />
  {/if}
  {#if welcomeUi.open}
    <WelcomeDialog />
  {/if}
  {#if showExport}
    <ExportDialog onClose={() => (showExport = false)} loop={transport.loop ?? null} />
  {/if}

  <!-- ステータスバー: 負荷・デバイス・MCP・版数(押すと設定・コピー) -->
  <footer>
    {#if info}
      <code class="path" title="プロジェクトフォルダ">{info.project_dir}</code>
    {/if}
    <div class="st">
      {#if transport.playing || dspWarn}
        <span
          class="it"
          class:warn={dspWarn}
          title={`音の処理の負荷(この再生の開始から)\n平均 ${dspAvg.toFixed(0)}% / 直近 3 秒の最大 ${dspMax.toFixed(0)}%\n処理落ち(Glaux の計算が間に合わない): ${dspCounts.overruns} 回\n呼び出し遅延(他の処理に CPU を奪われた): ${dspCounts.late} 回\nOS が知らせた音切れ: ${dspCounts.xruns} 回\n再生データの差し替え(編集で音が切り直される): ${dspCounts.swaps} 回${transport.dsp?.realtime_denied ? "\nOS がリアルタイム優先度を認めていません(途切れやすくなります)" : ""}${dspDrops > 0 ? "\n途切れるときは、設定 → オーディオでバッファを大きくしてください" : ""}`}
          ><Icon name="cpu" size={12} />{dspAvg.toFixed(0)}%{#if dspDrops > 0}
            <Icon name="triangle-alert" size={12} />{dspDrops}{/if}</span
        >
      {/if}
      {#if audioDev}
        <button class="it" onclick={() => openSettings("audio")} title="出力デバイス(クリックで設定)"
          ><Icon name="speaker" size={12} />{audioDev.output ?? "なし"}{audioDev.rate
            ? ` · ${(audioDev.rate / 1000).toFixed(1)} kHz`
            : ""}</button
        >
        <button class="it" onclick={() => openSettings("audio")} title="入力デバイス(クリックで設定)"
          ><Icon name="mic" size={12} />{audioDev.input ?? "なし"}</button
        >
        <button class="it" onclick={() => openSettings("midi")} title="MIDI 入力(クリックで設定)"
          ><Icon name="keyboard-music" size={12} />{audioDev.midi ?? "なし"}</button
        >
      {/if}
      {#if info}
        <button class="it" onclick={copyMcpUrl} title={`MCP サーバー ${info.mcp_url}\nクリックで Claude Code への登録コマンドをコピー`}
          ><Icon name={mcpCopied ? "check" : "link"} size={12} />{mcpCopied ? "コピーしました" : "MCP"}</button
        >
      {/if}
      <span class="it" title="版数(編集・取り消し・やり直しのたびに増える)">v{projectVersion}</span>
    </div>
  </footer>
</div>

<Toasts />

<style>
  .layout {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  /* ---- ヘッダー(左 / 中央 / 右の 3 つ。中央は常に真ん中) ---- */
  header {
    height: 52px;
    flex-shrink: 0;
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto minmax(0, 1fr);
    align-items: center;
    gap: var(--sp-3);
    padding: 0 var(--sp-3);
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
    white-space: nowrap;
  }

  .h-left,
  .h-right {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    min-width: 0;
  }

  .h-right {
    justify-content: flex-end;
  }

  .h-center {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
  }

  /* タイムライン / ミキサーの切り替え(2 つだけの切り替えなので、つながったボタン) */
  .view-switch {
    display: flex;
    margin-left: var(--sp-2);
    border: 1px solid var(--border);
    border-radius: var(--r-md);
    overflow: hidden;
  }

  .view-switch button {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 26px;
    padding: 0 10px;
    border: 0;
    border-radius: 0;
    background: none;
    color: var(--text-dim);
    font-size: var(--fs-sm);
    --icon-size: 14px;
  }

  .view-switch button + button {
    border-left: 1px solid var(--border);
  }

  .view-switch button.on {
    background: color-mix(in srgb, var(--accent) 14%, var(--bg-panel));
    color: var(--accent);
  }

  /* ブランドガイド: アプリ内では白フチの全身版を使い、影・発光は加えない */
  .owl {
    display: block;
    width: 28px;
    height: 28px;
    flex-shrink: 0;
  }

  .tp {
    display: flex;
    gap: 2px;
  }

  .tp .btn {
    height: 30px;
    width: 30px;
  }

  .tp .play {
    width: 38px;
  }

  .rec {
    color: var(--danger);
  }

  .rec-on {
    border-color: var(--danger);
    background: color-mix(in srgb, var(--danger) 25%, var(--bg-panel));
    animation: rec-blink 1s ease-in-out infinite;
  }

  .monitor-on {
    background: #5a4520;
    border-color: #e0b050;
    color: #ffe2a8;
  }

  @keyframes rec-blink {
    50% {
      background: color-mix(in srgb, var(--danger) 55%, var(--bg-panel));
    }
  }

  /* 表示窓 */
  .lcd {
    display: flex;
    align-items: stretch;
    height: 36px;
    background: var(--bg-inset);
    border: 1px solid var(--border);
    border-radius: var(--r-md);
    overflow: hidden;
  }

  .lcd .f {
    display: flex;
    flex-direction: column;
    justify-content: center;
    align-items: flex-start;
    padding: 0 10px;
    border: 0;
    border-left: 1px solid var(--border);
    border-radius: 0;
    background: none;
    color: var(--text);
    min-width: 0;
    height: auto;
  }

  .lcd .f:first-child {
    border-left: 0;
  }

  .lcd .lbl {
    font-size: 9px;
    line-height: 1;
    color: var(--text-faint);
    letter-spacing: 0.04em;
  }

  .lcd .val {
    font-family: var(--mono);
    font-size: 14px;
    line-height: 1.25;
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }

  .lcd .pos .val {
    color: var(--accent);
    font-size: 16px;
    min-width: 5.5ch;
  }

  .lcd.recording .pos .val {
    color: var(--danger);
  }

  .lcd .key .val {
    font-family: inherit;
    font-size: var(--fs-md);
  }

  .lcd .edit {
    cursor: pointer;
  }

  .lcd .edit:hover {
    background: var(--bg-raised);
    color: var(--text);
  }

  .lcd .edit .val::after {
    content: " ▾";
    font-size: 9px;
    color: var(--text-faint);
  }

  .lcd .rec-lbl {
    color: var(--danger);
  }

  .lcd-input {
    width: 64px;
    height: 20px;
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--accent-dim);
    border-radius: var(--r-sm);
    padding: 0 4px;
    font-family: var(--mono);
    font-size: var(--fs-md);
  }

  .sig-edit {
    display: flex;
    align-items: center;
    gap: 2px;
    color: var(--text-dim);
  }

  .sig-input {
    width: 40px;
  }

  .sig-den {
    height: 20px;
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--accent-dim);
    border-radius: var(--r-sm);
    font-size: var(--fs-md);
  }

  .rec-meter {
    display: inline-block;
    position: relative;
    width: 70px;
    height: 6px;
    margin-top: 4px;
    border-radius: 3px;
    background: var(--bg-raised);
    overflow: hidden;
  }

  .rec-meter-fill {
    position: absolute;
    inset: 0 auto 0 0;
    background: var(--ok);
  }

  .rec-meter-fill.hot {
    background: var(--danger);
  }

  .sep {
    width: 1px;
    height: 22px;
    background: var(--border);
    flex-shrink: 0;
  }

  .ai-indicator {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 26px;
    padding: 0 10px;
    border-radius: 13px;
    background: color-mix(in srgb, var(--ai) 16%, transparent);
    color: var(--ai);
    font-size: var(--fs-sm);
    min-width: 0;
    animation: ai-pulse 1.2s ease-in-out infinite;
  }

  .ai-text {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* 考え中(呼び出しの合間)は控えめに、ゆっくり */
  .ai-indicator.thinking {
    opacity: 0.7;
    animation-duration: 2.4s;
  }

  @keyframes ai-pulse {
    50% {
      background: color-mix(in srgb, var(--ai) 6%, transparent);
    }
  }

  /* 狭い画面(ノート PC の 150% 表示など)では文字を減らす */
  @media (max-width: 1280px) {
    .export-label,
    .lcd .key {
      display: none;
    }
  }

  @media (max-width: 1150px) {
    .ai-text {
      display: none;
    }
  }

  /* ---- 本体 ---- */
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

  .mixer-host {
    flex: 1;
    min-height: 0;
    position: relative;
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
    height: 5px;
    flex-shrink: 0;
    cursor: row-resize;
    background: var(--border);
    touch-action: none;
  }

  /* 下のパネルを隠している間は高さを変えられない(見えない領域の高さだけ変わっていた) */
  .row-handle.collapsed {
    cursor: default;
  }

  .collapse-btn {
    position: absolute;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    z-index: 2;
    height: 16px;
    padding: 0 10px;
    font-size: 10px;
    border-radius: 8px;
    --icon-size: 12px;
  }

  .row-handle:not(.collapsed):hover,
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

  /* ---- ステータスバー ---- */
  footer {
    height: 26px;
    padding: 0 var(--sp-3);
    background: var(--bg-panel);
    border-top: 1px solid var(--border);
    color: var(--text-dim);
    font-size: var(--fs-xs);
    flex-shrink: 0;
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: var(--sp-3);
  }

  footer .path {
    font-size: var(--fs-xs);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  footer .st {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    min-width: 0;
    overflow: hidden;
  }

  footer .it {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    border: 0;
    background: none;
    padding: 0;
    font-size: var(--fs-xs);
    color: var(--text-dim);
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
    max-width: 240px;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  footer button.it:hover {
    color: var(--text);
  }

  footer .it.warn {
    color: var(--warn);
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
