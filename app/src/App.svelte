<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "./lib/api";
  import { barAtTick, buildBars, nextBarHead, prevBarHead } from "./lib/barMap";
  import type { AppInfo, EntrySummary, Project, TransportState } from "./lib/types";
  import Timeline from "./lib/Timeline.svelte";
  import HistoryPanel from "./lib/HistoryPanel.svelte";
  import ChatPanel from "./lib/ChatPanel.svelte";
  import ProjectMenu from "./lib/ProjectMenu.svelte";
  import PianoRoll from "./lib/PianoRoll.svelte";
  import SettingsPanel from "./lib/SettingsPanel.svelte";
  import { applyTheme } from "./lib/settings.svelte";
  import { chatStatus } from "./lib/aiStatus.svelte";
  import { pianoRollStore } from "./lib/selection.svelte";

  let project = $state<Project | null>(null);
  let projectVersion = $state(0);
  let entries = $state<EntrySummary[]>([]);
  let info = $state<AppInfo | null>(null);
  let error = $state<string | null>(null);
  let mcpCopied = $state(false);
  let transport = $state<TransportState>({ available: false, playing: false, tick: 0 });
  let showSettings = $state(false);
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

  const toolLabels: Record<string, string> = {
    get_project: "プロジェクトを読んでいます",
    get_history: "履歴を確認しています",
    apply_commands: "編集しています",
    undo: "取り消しています",
    redo: "やり直しています",
    checkpoint: "チェックポイントを作成しています",
    revert_to: "巻き戻しています",
  };

  async function refresh() {
    try {
      const [p, h] = await Promise.all([api.getProject(), api.getHistory()]);
      project = p.project;
      projectVersion = p.project_version;
      entries = h.entries;
      error = null;
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

  onMount(() => {
    applyTheme();
    api.appInfo().then((i) => (info = i));
    refresh();

    let idleTimer: ReturnType<typeof setTimeout> | undefined;
    const unlistenChanged = api.onProjectChanged(scheduleRefresh).catch((e) => {
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
    let pollTick = 0;
    const transportTimer = setInterval(async () => {
      pollTick += 1;
      if (!transport.playing && pollTick % 4 !== 0) return;
      try {
        transport = await api.transportState();
      } catch {
        // 起動直後など。次のポーリングで回復する
      }
    }, 100);

    // キーボード操作(入力欄にフォーカスがあるときは除く)
    // Space: 再生/一時停止, ←/→: 前/次の小節頭, Home/End: 先頭/終端
    const onKeydown = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const typing =
        target && (target.tagName === "TEXTAREA" || target.tagName === "INPUT");
      if (typing) return;
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
      clearInterval(transportTimer);
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
      transport = await api.transportState();
    } catch (e) {
      error = String(e);
    }
  }

  async function stopPlayback() {
    if (!transport.available) return;
    try {
      await api.transportStop();
      transport = await api.transportState();
    } catch (e) {
      error = String(e);
    }
  }

  async function seek(tick: number) {
    if (!transport.available) return;
    try {
      await api.transportSeek(tick);
      transport = await api.transportState();
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
      <span class="owl">🦉</span>
      <ProjectMenu title={project?.meta.title ?? "…"} />
    </div>
    <div class="transport">
      <button onclick={seekStart} disabled={!transport.available} title="先頭へ(Home)">⏮</button>
      <button onclick={prevBar} disabled={!transport.available} title="前の小節頭へ(←)">⏪</button>
      <button
        class="play"
        onclick={togglePlay}
        disabled={!transport.available}
        title={transport.available
          ? "再生/一時停止(スペースキー)"
          : "オーディオデバイスが利用できません"}
      >
        {transport.playing ? "⏸" : "▶"}
      </button>
      <button onclick={stopPlayback} disabled={!transport.available} title="停止(先頭に戻る)">
        ⏹
      </button>
      <button onclick={nextBar} disabled={!transport.available} title="次の小節頭へ(→)">⏩</button>
      <button onclick={seekEnd} disabled={!transport.available} title="終端へ(End)">⏭</button>
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
      <span class="stat" title="適用済み履歴エントリ数">v{projectVersion}</span>
      <button onclick={doUndo} title="直前の編集を取り消す">↶ Undo</button>
      <button onclick={doRedo} title="やり直す">↷ Redo</button>
      <div class="master" title="マスター音量">
        <span class="master-icon">🔊</span>
        <input
          type="range"
          min="-40"
          max="6"
          step="0.5"
          value={project?.master.volume_db ?? 0}
          onchange={setMasterVolume}
        />
        <span class="stat">{(project?.master.volume_db ?? 0).toFixed(1)} dB</span>
      </div>
      <button onclick={doExport} disabled={exporting} title="WAV に書き出す(プロジェクト内 export フォルダ)">
        {exporting ? "書き出し中…" : "⬇ WAV"}
      </button>
      <button onclick={() => (showSettings = true)} title="設定">⚙</button>
    </div>
    {#if indicator !== "idle"}
      <div class="ai-indicator" class:thinking={indicator === "session"}>
        <span class="pulse"></span>
        {#if indicator === "calling"}
          AI が{toolLabels[aiTool] ?? aiTool}…
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
        <PianoRoll
          {project}
          playheadTick={transport.tick}
          playing={transport.playing}
          onSeek={seek}
        />
      {:else}
        <div class="loading">読み込み中…</div>
      {/if}
    </section>
    <div class="row-handle" onpointerdown={startRowResize} title="ドラッグで高さを調整"></div>
    <section class="bottom-area" style="height:{bottomHeight}px">
      <div class="chat-section" style="flex:0 0 {chatFrac * 100}%">
        <ChatPanel />
      </div>
      <div class="col-handle" onpointerdown={startColResize} title="ドラッグで幅を調整"></div>
      <div class="history-section">
        <HistoryPanel {entries} />
      </div>
    </section>
  </main>

  {#if showSettings}
    <SettingsPanel onClose={() => (showSettings = false)} />
  {/if}

  <footer>
    {#if exportMsg}
      <code class="export-msg">{exportMsg}</code>
    {:else if info}
      <code title="プロジェクトフォルダ">{info.project_dir}</code>
    {/if}
  </footer>
</div>

<style>
  .layout {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  header {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 8px 14px;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 8px;
    font-weight: 600;
    font-size: 15px;
  }

  .owl {
    filter: drop-shadow(0 0 4px var(--accent));
  }

  .transport {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .play {
    min-width: 44px;
    font-size: 14px;
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

  .mcp {
    margin-left: auto;
    min-width: 0;
  }

  .mcp-url {
    font-family: Consolas, monospace;
    font-size: 12px;
    color: var(--text-dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 360px;
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
  }

  .export-msg {
    color: var(--accent);
  }

  .error {
    background: #5c2b33;
    color: #ffb4c0;
    padding: 6px 14px;
  }

  .loading {
    padding: 40px;
    color: var(--text-dim);
  }
</style>
