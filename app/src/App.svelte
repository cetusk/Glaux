<script lang="ts">
  import { onMount, untrack } from "svelte";
  import * as api from "./lib/api";
  import { shouldYieldKey } from "./lib/keys";
  import { toolDoing } from "./lib/toolLabels";
  import Toasts from "./lib/Toasts.svelte";
  import Icon from "./lib/Icon.svelte";
  import { showToast } from "./lib/toast.svelte";
  import { refreshHarmony } from "./lib/harmony.svelte";
  import { barAtTick, buildBars, nextBarHead, prevBarHead } from "./lib/barMap";
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
  import DesignView from "./lib/DesignView.svelte";
  import PlanHistoryPanel from "./lib/PlanHistoryPanel.svelte";
  import { designRedo, designUndo, designWatch, refreshClipStates, refreshDesign } from "./lib/design.svelte";
  import TransportLcd from "./lib/TransportLcd.svelte";
  import { abLooping } from "./lib/abLoop";
  import StatusBar from "./lib/StatusBar.svelte";
  import { applyTheme, openSettings, saveSettings, settings, settingsUi, welcomeUi } from "./lib/settings.svelte";
  import { chatStatus } from "./lib/aiStatus.svelte";
  import {
    inspectorStore,
    midiArmStore,
    pianoRollStore,
    projectRev,
    selectionStore,
    soundDesignStore,
    viewStore,
  } from "./lib/selection.svelte";

  // 曲は丸ごと差し替えて更新する(深いリアクティブにすると、全体を取り直すたびに全ノートがプロキシになる)。
  // 中を直接書き換えても画面は変わらないので、変えるときは新しいオブジェクトを代入する
  let project = $state.raw<Project | null>(null);
  let projectVersion = $state(0);
  let entries = $state<EntrySummary[]>([]);
  let historyTotal = $state(0);
  let redoable = $state<EntrySummary[]>([]);
  let info = $state<AppInfo | null>(null);
  let error = $state<string | null>(null);
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
      projectRev.value += 1;
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
    // 履歴だけの変更(チェックポイント)なら曲は取り直さず、履歴の表示だけを更新する
    if (evs.length > 0 && evs.every((e) => e.history_only) && evs[0].project_version === projectVersion + 1) {
      const seq = ++syncSeq;
      try {
        const h = await api.getHistory();
        if (seq !== syncSeq) return;
        projectVersion = evs[evs.length - 1].project_version;
        entries = h.entries;
        historyTotal = h.total;
        redoable = h.redoable ?? [];
      } catch {
        refresh();
      }
      return;
    }
    const partial = changedTracks(evs);
    if (!partial) return refresh();
    const seq = ++syncSeq;
    try {
      const [t, h] = await Promise.all([api.getTracks([...partial.ids]), api.getHistory()]);
      if (seq !== syncSeq) return;
      const p = project;
      if (!p || t.project_version !== partial.version || t.tracks.length !== partial.ids.size) return refresh();
      const tracks = [...p.tracks];
      for (const nt of t.tracks) {
        const i = tracks.findIndex((x) => x.id === nt.id);
        if (i < 0) return refresh();
        tracks[i] = nt;
      }
      // 変わらないトラックは同じオブジェクトのまま(描き直しの判定がそのまま効く)
      project = { ...p, tracks };
      projectVersion = t.project_version;
      projectRev.value += 1;
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

  // 設計画面の中身(開いているとき)と、タイムラインのクリップの印の読み直し。
  // 設計画面の中身は盛り上がりなどを測るので重い。曲・計画が変わってから 300ms 落ち着いたら 1 回だけ
  let designTimer: ReturnType<typeof setTimeout> | undefined;
  function scheduleDesign(delay = 300) {
    clearTimeout(designTimer);
    designTimer = setTimeout(() => {
      // 設計画面の外でも、チャットに案の一覧があれば読み直す(採用・取り消しで一覧の状態が変わる)
      if (viewStore.main === "design" || designWatch.chat > 0) refreshDesign();
      else refreshClipStates();
    }, delay);
  }
  $effect(() => {
    // 画面を切り替えたら(設計画面を開いたら)すぐ読み直す
    void viewStore.main;
    scheduleDesign(0);
  });
  /// 下の履歴の欄のタブ(設計画面では計画の履歴を先に出す)
  let historyTab = $state<"plan" | "song">("plan");

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
    api.appInfo().then((i) => {
      info = i;
      // 別の Glaux と重なったときの知らせ(曲を開けなかった・窓口のポートを変えた)は長めに出す
      for (const line of (i.startup_notice ?? "").split("\n").filter((l) => l.trim())) {
        showToast("warn", line, { ms: 20000 });
      }
    });
    refresh();
    // 前回選んだオーディオデバイスに戻す(抜かれていたら既定のまま)
    (async () => {
      if (settings.outputVolumeDb !== 0) {
        await api.setOutputVolume(settings.outputVolumeDb).catch(() => {});
      }
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
      // 計画だけの変更は曲の版が進まない。曲の再取得の判断(版の連続)を乱さないよう、曲は取り直さず、
      // 設計画面とクリップの印だけを読み直す
      scheduleDesign();
      if (ev?.plans_version != null) return;
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

  // ループ中に範囲選択・曲の長さが変わったら区間を更新。ループの入り切りそのものでは張り直さない
  // (聴き比べが範囲をループにすると loopOn が立ち、ここで曲全体に張り直して聴き比べのループが外れていた)。
  // 聴き比べの間は、聴き比べの範囲のままにする
  $effect(() => {
    void selectionStore.range;
    void contentEndTick;
    if (!untrack(() => loopOn) || abLooping()) return;
    const range = loopRange();
    if (range) api.transportSetLoop(range.start, range.end).catch(() => {});
  });

  // ---- 聴く音量(アプリから鳴る音だけ。曲・書き出しには入らない) ----
  function fmtListen(db: number): string {
    return db <= -60 ? "無音" : `${db > 0 ? "+" : ""}${db} dB`;
  }

  function setListenVolume(db: number, save: boolean) {
    settings.outputVolumeDb = db;
    api.setOutputVolume(db).catch(() => {});
    if (save) saveSettings();
  }

  async function doUndo() {
    // 設計画面では、設計画面で直した先(曲か計画)の履歴を戻す(覚えが無ければ計画)
    if (viewStore.main === "design") return designUndo();
    try {
      await api.undo();
    } catch (e) {
      error = String(e);
    }
  }

  async function doRedo() {
    if (viewStore.main === "design") return designRedo();
    try {
      await api.redo();
    } catch (e) {
      error = String(e);
    }
  }

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
          aria-label="タイムライン"
          title="タイムライン(曲の流れ・クリップ・ピアノロール)"><Icon name="rows-2" /><span class="lbl">タイムライン</span></button
        >
        <button
          role="tab"
          aria-selected={viewStore.main === "mixer"}
          class:on={viewStore.main === "mixer"}
          onclick={() => (viewStore.main = "mixer")}
          aria-label="ミキサー"
          title="ミキサー(音量・パン・送り・エフェクトのつなぎ方)"><Icon name="sliders-horizontal" /><span class="lbl">ミキサー</span></button
        >
        <button
          role="tab"
          aria-selected={viewStore.main === "design"}
          class:on={viewStore.main === "design"}
          onclick={() => (viewStore.main = "design")}
          aria-label="設計"
          title="設計(曲の計画と実際: 盛り上がり・パートごとの音域・パートの役割)"><Icon name="spline" /><span class="lbl">設計</span></button
        >
      </div>
    </div>
    <div class="h-center">
      <div class="tp">
        <button class="btn icon" onclick={seekStart} disabled={!transport.available} title="先頭へ(Home)" aria-label="先頭へ"
          ><Icon name="skip-back" /></button
        >
        <button class="btn icon bar-step" onclick={prevBar} disabled={!transport.available} title="前の小節へ(←)" aria-label="前の小節へ"
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
        <button class="btn icon bar-step" onclick={nextBar} disabled={!transport.available} title="次の小節へ(→)" aria-label="次の小節へ"
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
      <TransportLcd {project} {contentEndTick} midiArmName={midiArm?.name ?? null} onError={(m) => (error = m)} />
    </div>
    <div class="h-right">
      {#if indicator !== "idle"}
        <div class="ai-indicator" class:thinking={indicator === "session"}>
          <Icon name="sparkles" size={14} />
          <span class="ai-text">{indicator === "calling" ? `AI が${toolDoing(aiTool)}…` : "AI が作業中です…"}</span>
        </div>
      {/if}
      <!-- 聴く音量: アプリから鳴る音だけ(曲のマスター音量・書き出しとは別) -->
      <div
        class="listen-vol"
        class:changed={settings.outputVolumeDb !== 0}
        title={`聴く音量 ${fmtListen(settings.outputVolumeDb)}(このアプリから鳴る音だけ。曲のマスター音量・書き出しには影響しません。ダブルクリックで 0 dB)`}
      >
        <Icon name="volume-2" size={15} />
        <input
          type="range"
          min="-60"
          max="6"
          step="1"
          value={settings.outputVolumeDb}
          disabled={!transport.available}
          oninput={(e) => setListenVolume(Number(e.currentTarget.value), false)}
          onchange={(e) => setListenVolume(Number(e.currentTarget.value), true)}
          ondblclick={() => setListenVolume(0, true)}
          aria-label="聴く音量(曲には影響しない)"
        />
      </div>
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
        {:else if viewStore.main === "design"}
          <div class="mixer-host">
            <DesignView {project} />
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
        <ChatPanel {project} />
      </div>
      <div class="col-handle" role="separator" aria-orientation="vertical" onpointerdown={startColResize} title="ドラッグで幅を調整"></div>
      <div class="history-section">
        {#if viewStore.main === "design"}
          <div class="history-tabs" role="tablist" aria-label="履歴の切り替え">
            <button role="tab" class="btn sm" class:on={historyTab === "plan"} aria-selected={historyTab === "plan"} onclick={() => (historyTab = "plan")}
              >計画の履歴</button
            >
            <button role="tab" class="btn sm" class:on={historyTab === "song"} aria-selected={historyTab === "song"} onclick={() => (historyTab = "song")}
              >曲の履歴</button
            >
          </div>
        {/if}
        {#if viewStore.main === "design" && historyTab === "plan"}
          <PlanHistoryPanel />
        {:else}
          <HistoryPanel {entries} total={historyTotal} {redoable} />
        {/if}
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

  <StatusBar {info} {audioDev} {projectVersion} />
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

  /* 左の欄が狭いときは、切り替えの名前を段階的にしまう(選んでいる画面は色で分かり、名前は説明の吹き出し)。
     切り替えは縮めず、足りなければ曲名を「…」で縮める */
  .h-left {
    container-type: inline-size;
  }
  /* 少し狭い: 選んでいる画面だけ名前を出す */
  @container (max-width: 470px) {
    .view-switch button:not(.on) .lbl {
      display: none;
    }
    .view-switch button:not(.on) {
      padding: 0 9px;
    }
  }
  /* かなり狭い: 3 つともアイコンだけ */
  @container (max-width: 290px) {
    .view-switch button .lbl {
      display: none;
    }
    .view-switch button {
      padding: 0 9px;
    }
  }
  /* さらに狭い: ロゴもしまう(切り替えと曲名のメニューを残す) */
  @container (max-width: 240px) {
    .h-left .owl {
      display: none;
    }
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
    flex-shrink: 0;
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

  .listen-vol {
    display: flex;
    align-items: center;
    gap: 4px;
    height: 28px;
    padding: 0 6px;
    min-width: 0;
    flex-shrink: 1;
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    color: var(--text-dim);
  }

  .listen-vol.changed {
    color: var(--text);
  }

  .listen-vol input {
    width: 72px;
    min-width: 36px;
    flex-shrink: 1;
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
    .export-label {
      display: none;
    }
  }

  @media (max-width: 1150px) {
    .ai-text {
      display: none;
    }
  }

  /* さらに狭い(画面の半分など): 1 行に収まらず、左右の欄が中央(再生・位置の表示)に重なっていた。
     2 段にして、上の段に左右の欄、下の段に中央を置く(どのボタンも隠さない) */
  @media (max-width: 1060px) {
    header {
      height: auto;
      grid-template-columns: minmax(0, 1fr) auto;
      grid-template-areas:
        "left right"
        "center center";
      row-gap: 4px;
      padding-block: 6px;
    }
    .h-left {
      grid-area: left;
    }
    .h-right {
      grid-area: right;
    }
    .h-center {
      grid-area: center;
      justify-content: center;
      min-width: 0;
    }
  }
  /* とても狭い: 前・次の小節のボタンをしまう(← → キーで動かせる。時間の表示もしまう: TransportLcd) */
  @media (max-width: 640px) {
    .bar-step {
      display: none;
    }
  }

  /* ---- 本体 ---- */
  .roll-split {
    position: absolute;
    inset: 0;
    /* タイムラインのマーカーの行(z-index 7)・小節の行より前に出す(見出しの行 = 閉じる・スナップが隠れていた)。
       メニュー(30)よりは後ろ */
    z-index: 8;
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

  .history-tabs {
    display: flex;
    gap: 4px;
    padding: 8px 10px 0;
  }

  .history-section {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
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
