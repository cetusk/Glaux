<script lang="ts">
  import Icon from "./Icon.svelte";
  import { onMount, tick } from "svelte";
  import * as api from "./api";
  import { chatStatus } from "./aiStatus.svelte";
  import { toolShort } from "./toolLabels";
  import { clearAiHighlight, setAiHighlight } from "./aiHighlight.svelte";
  import { renderMarkdown } from "./markdown";
  import { showError, showToast } from "./toast.svelte";
  import {
    CHAT_MODELS,
    CHAT_PROVIDERS,
    openSettings,
    playDoneChime,
    playErrorChime,
    saveSettings,
    setChatModel,
    settings,
  } from "./settings.svelte";

  const PROVIDERS = CHAT_PROVIDERS;
  const MODELS = CHAT_MODELS;
  const provider = $derived(settings.chatProvider);
  const currentModel = $derived(provider === "codex" ? settings.chatCodexModel : settings.chatModel);
  const isCustomModel = $derived(!MODELS[provider].some((m) => m.value === currentModel));

  function pickProvider(e: Event) {
    const v = (e.currentTarget as HTMLSelectElement).value;
    settings.chatProvider = v === "codex" ? "codex" : "claude";
    saveSettings();
  }

  function pickModel(e: Event) {
    const el = e.currentTarget as HTMLSelectElement;
    const v = el.value;
    if (v === "__custom") {
      // 名前を入れるのは設定の「AI」のページで(以前は window.prompt だった)
      el.value = isCustomModel ? "__current" : currentModel;
      openSettings("ai");
      return;
    }
    setChatModel(v);
  }
  import { MASTER_FOCUS_ID, pianoRollStore, selectionStore, soundDesignStore } from "./selection.svelte";

  interface Msg {
    role: "user" | "assistant" | "tool" | "notice" | "error" | "turn";
    text: string;
    /** role "turn": ターンの開始前の最後の履歴エントリ(取り消しの起点)と、取り消したか */
    since?: string | null;
    reverted?: boolean;
    /** role "user": 送信待ちのまま停止したので送らなかった */
    dropped?: boolean;
  }

  /// 送信待ちの指示(実行中に書いたもの)。会話の下に並べ、前のターンが終わったら順に送る
  interface Pending {
    text: string;
    fullPrompt: string;
  }
  let pending = $state<Pending[]>([]);

  /// 実行中のターンの開始前の最後の履歴エントリ(null = 履歴が空)
  let turnStart: string | null = null;

  /// ターンが終わったら、AI の編集の件数を数えて「取り消す」を出し、変わった所を縁取る
  async function finishTurn() {
    try {
      const ch = await api.turnChanges(turnStart);
      if (ch.entry_ids.length === 0) return;
      setAiHighlight(ch);
      push({ role: "turn", text: `このターンの編集 ${ch.entry_ids.length} 件`, since: turnStart });
    } catch {
      // 数えられなくても会話には影響しない
    }
  }

  async function revertTurnAt(m: Msg) {
    if (m.reverted) return;
    try {
      const r = await api.revertTurn(m.since ?? null);
      m.reverted = true;
      saveLog();
      clearAiHighlight();
      showToast(
        r.conflicts.length > 0 ? "warn" : "ok",
        r.conflicts.length > 0
          ? `${r.reverted} 件を取り消しました。後から同じ所を触った編集があります(履歴で確認してください)`
          : `このターンの編集 ${r.reverted} 件を取り消しました(Ctrl+Z で取り消しを戻せます)`,
      );
    } catch (e) {
      showError("このターンを取り消せませんでした", e);
    }
  }


  let messages = $state<Msg[]>([]);
  let input = $state("");

  // ---- 送った指示の履歴(入力欄が空のとき ↑ でさかのぼる。ターミナルと同じ) ----
  const MAX_PROMPTS = 100;
  let prompts: string[] = [];
  /** いま入力欄に出している履歴の位置(null = 履歴をたどっていない) */
  let recall: number | null = null;

  /** ↑↓ で履歴をたどる。たどれたら true(キーの既定の動きを止める) */
  function recallPrompt(dir: -1 | 1): boolean {
    // 空の欄か、たどって出した文をそのまま(手を加えずに)出しているときだけ
    const browsing = recall !== null && input === prompts[recall];
    if (!browsing && input !== "") return false;
    if (!browsing) recall = null;
    if (dir === -1) {
      if (prompts.length === 0) return false;
      recall = recall === null ? prompts.length - 1 : Math.max(0, recall - 1);
    } else {
      if (recall === null) return false;
      recall = recall + 1 < prompts.length ? recall + 1 : null;
    }
    input = recall === null ? "" : prompts[recall];
    // カーソルは文末へ
    tick().then(() => {
      if (inputEl) inputEl.selectionStart = inputEl.selectionEnd = input.length;
    });
    return true;
  }

  function rememberPrompt(prompt: string) {
    if (prompts[prompts.length - 1] !== prompt) prompts.push(prompt);
    if (prompts.length > MAX_PROMPTS) prompts = prompts.slice(-MAX_PROMPTS);
    recall = null;
  }

  // ---- 会話ログの保存と復元(プロジェクトの cache/chat-log.json) ----
  const MAX_SAVED = 400;
  /** ログを読んだプロジェクトのフォルダ(null = まだ読んでいない。読むまで保存しない) */
  let logDir: string | null = null;
  let saveTimer: ReturnType<typeof setTimeout> | undefined;

  function saveLog() {
    clearTimeout(saveTimer);
    const dir = logDir;
    if (dir === null) return;
    saveTimer = setTimeout(() => {
      const kept = messages.slice(-MAX_SAVED);
      const log = JSON.stringify({ version: 1, messages: kept, prompts });
      api.saveChatLog(dir, log).catch(() => undefined);
    }, 300);
  }

  async function loadLog() {
    logDir = null;
    pending = [];
    try {
      const r = await api.loadChatLog();
      const saved = r.log ? JSON.parse(r.log) : null;
      messages = Array.isArray(saved?.messages) ? (saved.messages as Msg[]) : [];
      prompts = Array.isArray(saved?.prompts) ? (saved.prompts as string[]).filter((p) => typeof p === "string") : [];
      logDir = r.dir;
    } catch {
      messages = [];
      prompts = [];
    }
    recall = null;
    stickToBottom = true;
    scrollToBottom(true);
  }

  // 入力欄は内容に合わせて高くなる(1 行〜5 行。それ以上は欄の中でスクロール)
  const MAX_LINES = 5;
  let inputEl: HTMLTextAreaElement | undefined = $state();
  $effect(() => {
    void input;
    const el = inputEl;
    if (!el) return;
    const cs = getComputedStyle(el);
    const line = parseFloat(cs.lineHeight) || 18;
    const extra =
      parseFloat(cs.paddingTop) +
      parseFloat(cs.paddingBottom) +
      parseFloat(cs.borderTopWidth) +
      parseFloat(cs.borderBottomWidth);
    el.style.height = "auto";
    const max = line * MAX_LINES + extra;
    el.style.height = `${Math.min(el.scrollHeight + parseFloat(cs.borderTopWidth) + parseFloat(cs.borderBottomWidth), max)}px`;
    el.style.overflowY = el.scrollHeight > max ? "auto" : "hidden";
  });
  let scroller: HTMLDivElement | undefined = $state();

  // プロジェクトを切り替えたら、そのプロジェクトの会話ログを出す(会話はプロジェクトごと)
  $effect(() => {
    void chatStatus.epoch;
    loadLog();
  });

  /// 下端の近くにいるときだけ、新しい発言で下へ送る(上を読み返している間は動かさない)
  let stickToBottom = true;
  function onScroll() {
    const el = scroller;
    if (!el) return;
    stickToBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
  }

  async function scrollToBottom(force = false) {
    if (!force && !stickToBottom) return;
    await tick();
    scroller?.scrollTo({ top: scroller.scrollHeight });
  }

  function push(msg: Msg, force = false) {
    messages.push(msg);
    scrollToBottom(force);
    saveLog();
  }

  async function send() {
    const prompt = input.trim();
    if (!prompt) return;

    // タイムラインの小節範囲・ピアノロールで開いているクリップを対象として指示に添える(マスク)
    const range = selectionStore.range;
    const focus = pianoRollStore.focus;
    let prefix = "";
    let shown = prompt;
    if (focus) {
      prefix +=
        `【対象クリップ(ユーザーがピアノロールで開いている)】トラック「${focus.trackName}」(${focus.trackId})の` +
        `クリップ「${focus.clipName}」(${focus.clipId})。特に指定がなければ、このクリップ内のノートへの操作として解釈してください。\n`;
      shown = `〔${focus.clipName}〕 ${shown}`;
    }
    if (range) {
      prefix +=
        `【対象範囲の指定(ユーザーが UI で選択)】小節 ${range.startBar + 1}〜${range.endBar + 1}` +
        `(tick ${range.startTick}〜${range.endTick})。` +
        `編集・分析はこの範囲内に限定し、範囲外のノートやクリップは変更しないでください。\n`;
      shown = `〔小節 ${range.startBar + 1}〜${range.endBar + 1}〕 ${shown}`;
    }
    const sd = soundDesignStore.focus;
    if (sd && sd.trackId === MASTER_FOCUS_ID) {
      prefix +=
        "【音作り中: マスターバス(ユーザーがマスターのエフェクトを開いている)】" +
        "エフェクトに関する指示は、特に指定がなければマスター(add_master_effect / set_master_param)が対象です。\n";
      shown = `〔音作り: マスター〕 ${shown}`;
    } else if (sd) {
      prefix +=
        `【音作り中のトラック(ユーザーが音作りビューで開いている)】「${sd.trackName}」(${sd.trackId})。` +
        `音色・エフェクトに関する指示は、特に指定がなければこのトラックが対象です。\n`;
      shown = `〔音作り: ${sd.trackName}〕 ${shown}`;
    }
    const fullPrompt = prefix ? `${prefix}\n${prompt}` : prompt;
    rememberPrompt(prompt);
    input = "";
    if (chatStatus.running || pending.length > 0) {
      // 実行中に書いた指示は、今のターンが終わってから送る
      pending.push({ text: shown, fullPrompt });
      scrollToBottom(true);
      return;
    }
    push({ role: "user", text: shown }, true);
    await start(fullPrompt);
  }

  /// 送信待ちの次の指示を送る(ターンが終わったとき)
  function sendNext() {
    const next = pending.shift();
    if (!next) return;
    push({ role: "user", text: next.text }, true);
    start(next.fullPrompt);
  }

  async function start(fullPrompt: string) {
    chatStatus.running = true;
    clearAiHighlight();
    try {
      const h = await api.getHistory(1);
      turnStart = h.entries.length > 0 ? h.entries[h.entries.length - 1].id : null;
    } catch {
      turnStart = null;
    }
    try {
      await api.sendChat(fullPrompt, currentModel, provider);
    } catch (e) {
      push({ role: "error", text: String(e) });
      chatStatus.running = false;
    }
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.isComposing) return;
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      send();
    } else if ((e.key === "ArrowUp" || e.key === "ArrowDown") && !e.shiftKey && !e.altKey && !e.ctrlKey && !e.metaKey) {
      if (recallPrompt(e.key === "ArrowUp" ? -1 : 1)) e.preventDefault();
    }
  }

  /// 送信待ちの指示を取り消す
  function dropPending(i: number) {
    pending.splice(i, 1);
  }

  async function cancel() {
    // 停止したら、送信待ちの指示も送らない(送らなかったことは会話に残す)
    for (const p of pending) messages.push({ role: "user", text: p.text, dropped: true });
    pending = [];
    saveLog();
    await api.cancelChat();
  }

  async function newConversation() {
    if (chatStatus.running) return;
    await api.resetChat();
    messages = [];
    pending = [];
    saveLog();
  }

  onMount(() => {
    const unlisten = api
      .onChatEvent((ev) => {
        switch (ev.kind) {
          case "started":
            break;
          case "assistant_text":
            push({ role: "assistant", text: ev.text });
            break;
          case "tool_use":
            push({ role: "tool", text: toolShort(ev.name) });
            break;
          case "result":
            chatStatus.running = false;
            // 成功時の最終テキストは assistant_text で受信済みなので出さない
            if (!ev.ok && ev.text) {
              push({ role: "error", text: ev.text });
            }
            if (ev.ok) playDoneChime();
            else playErrorChime();
            finishTurn().then(sendNext);
            break;
          case "notice":
            push({ role: "notice", text: ev.text });
            break;
          case "error":
            chatStatus.running = false;
            push({ role: "error", text: ev.message });
            playErrorChime();
            sendNext();
            break;
        }
      })
      .catch(() => undefined);
    return () => {
      unlisten.then((f) => f && f());
    };
  });
</script>

<div class="chat">
  <div class="chat-head">
    <h2><Icon name="sparkles" size={15} />AI に指示</h2>
    <div class="head-right">
      <select
        class="provider"
        value={provider}
        onchange={pickProvider}
        disabled={chatStatus.running}
        title="チャットの相手(Claude = Claude Code、GPT = Codex CLI。どちらもインストールしてログインしておく)。切り替えると新しい会話になります"
      >
        {#each PROVIDERS as p (p.value)}
          <option value={p.value}>{p.label}</option>
        {/each}
      </select>
      <select
        class="model"
        value={isCustomModel ? "__current" : currentModel}
        onchange={pickModel}
        disabled={chatStatus.running}
        title="AI のモデル(次の指示から反映。会話の文脈はそのまま引き継がれます)"
      >
        {#each MODELS[provider] as m (m.value)}
          <option value={m.value}>{m.label}</option>
        {/each}
        {#if isCustomModel}
          <option value="__current">{currentModel}</option>
        {/if}
        <option value="__custom">その他(モデル名を入力)…</option>
      </select>
      <button class="btn sm" onclick={newConversation} disabled={chatStatus.running} title="会話の文脈をリセットする(表示中の会話も消えます)"
        ><Icon name="message-square-plus" />新しい会話</button
      >
    </div>
  </div>

  <div class="messages" bind:this={scroller} onscroll={onScroll}>
    {#if messages.length === 0}
      <div class="hint">
        例:「4小節のベースラインを作って」「もっと音を明るくして」「さっきの編集を取り消して」
      </div>
    {/if}
    {#each messages as m}
      {#if m.role === "tool"}
        <div class="msg tool"><Icon name="sparkles" size={12} />{m.text}</div>
      {:else if m.role === "turn"}
        <div class="msg turn">
          <span>{m.text}{m.reverted ? "(取り消し済み)" : ""}</span>
          {#if !m.reverted}
            <button
              class="btn sm"
              onclick={() => revertTurnAt(m)}
              title="このターンで AI が行った編集をまとめて打ち消す(後から人間が行った編集は残す)"
              ><Icon name="rotate-ccw" />このターンを取り消す</button
            >
          {/if}
        </div>
      {:else if m.role === "assistant"}
        <div class="msg assistant md">{@html renderMarkdown(m.text)}</div>
      {:else if m.role === "user" && m.dropped}
        <div class="msg user dropped">
          <span class="struck">{m.text}</span>
          <div class="queue-note">停止したので送っていません</div>
        </div>
      {:else}
        <div class="msg {m.role}">{m.text}</div>
      {/if}
    {/each}
    {#if chatStatus.running}
      <div class="msg thinking"><span class="dots"></span></div>
    {/if}
    {#each pending as p, i}
      <div class="msg user waiting">
        {p.text}
        <div class="queue-note">
          送信待ち(今の指示が終わったら送ります)
          <button class="chip-x" onclick={() => dropPending(i)} title="この指示を送らない" aria-label="この指示を送らない"
            ><Icon name="x" size={11} /></button
          >
        </div>
      </div>
    {/each}
  </div>

  <!-- 指示の対象(ピアノロールのクリップ・インスペクターのトラック・選んだ小節)。✕ で外す -->
  {#if pianoRollStore.focus || soundDesignStore.focus || selectionStore.range}
    <div class="chips">
      {#if pianoRollStore.focus}
        <span class="chip" title="ピアノロールで開いているクリップが指示の対象になります"
          ><Icon name="piano" size={12} />{pianoRollStore.focus.clipName}<button
            class="chip-x"
            onclick={() => (pianoRollStore.focus = null)}
            title="ピアノロールを閉じる"
            aria-label="ピアノロールを閉じる"><Icon name="x" size={11} /></button
          ></span
        >
      {/if}
      {#if soundDesignStore.focus}
        <span
          class="chip"
          title={soundDesignStore.focus.trackId === MASTER_FOCUS_ID
            ? "エフェクトの指示はマスターへ"
            : "音色・エフェクトの指示はこのトラックへ"}
          ><Icon name="sliders-horizontal" size={12} />{soundDesignStore.focus.trackName} の音作り<button
            class="chip-x"
            onclick={() => (soundDesignStore.focus = null)}
            title="インスペクターを閉じる"
            aria-label="インスペクターを閉じる"><Icon name="x" size={11} /></button
          ></span
        >
      {/if}
      {#if selectionStore.range}
        <span class="chip" title="この範囲に限定して指示されます"
          ><Icon name="ruler" size={12} />小節 {selectionStore.range.startBar + 1}〜{selectionStore.range.endBar + 1}<button
            class="chip-x"
            onclick={() => (selectionStore.range = null)}
            title="範囲の指定を外す"
            aria-label="範囲の指定を外す"><Icon name="x" size={11} /></button
          ></span
        >
      {/if}
    </div>
  {/if}

  <div class="input-row">
    <textarea
      bind:this={inputEl}
      rows="2"
      placeholder={chatStatus.running
        ? "次の指示を書けます(今の指示が終わったら送ります)"
        : "AI への指示を入力(Enter で送信 / Shift+Enter で改行 / ↑ で前の指示)"}
      bind:value={input}
      onkeydown={onKeydown}
    ></textarea>
    {#if chatStatus.running}
      {#if input.trim()}
        <button class="send" onclick={send} title="今の指示が終わったら送ります">予約</button>
      {/if}
      <button class="stop" onclick={cancel} title="実行中の指示を中断する(送信待ちの指示も送りません)">停止</button>
    {:else}
      <button class="send" onclick={send} disabled={!input.trim()}>送信</button>
    {/if}
  </div>
</div>

<style>
  .chat {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }

  .chat-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 10px 10px 6px;
  }

  .head-right {
    display: flex;
    gap: 6px;
    align-items: center;
  }

  .model,
  .provider {
    height: 22px;
    max-width: 130px;
    background: var(--bg-inset);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    font-size: var(--fs-sm);
    padding: 0 4px;
  }

  h2 {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--fs-md);
    margin: 0;
    color: var(--text-dim);
  }

  h2 :global(.icon) {
    color: var(--ai);
  }

  .messages {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 4px 10px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .hint {
    color: var(--text-dim);
    font-size: 12px;
    line-height: 1.6;
    padding: 8px 0;
  }

  .msg {
    border-radius: 8px;
    padding: 7px 10px;
    font-size: 13px;
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-word;
    max-width: 95%;
  }

  .msg.user {
    align-self: flex-end;
    background: color-mix(in srgb, var(--human) 25%, var(--bg));
  }

  .msg.assistant {
    align-self: flex-start;
    background: var(--bg);
    border: 1px solid var(--border);
    border-left: 3px solid var(--ai);
  }

  .msg.user.waiting {
    opacity: 0.7;
    border: 1px dashed var(--border);
  }

  .msg.user.dropped {
    opacity: 0.5;
  }

  .struck {
    text-decoration: line-through;
  }

  .queue-note {
    display: flex;
    align-items: center;
    gap: 4px;
    margin-top: 3px;
    font-size: var(--fs-xs);
    color: var(--text-dim);
    text-decoration: none;
  }

  /* Markdown の返答: 段落などの間を詰め、改行は Markdown に任せる */
  .msg.md {
    white-space: normal;
  }

  .msg.md :global(p),
  .msg.md :global(ul),
  .msg.md :global(ol),
  .msg.md :global(pre),
  .msg.md :global(blockquote),
  .msg.md :global(.md-table) {
    margin: 0 0 6px;
  }

  .msg.md :global(:last-child) {
    margin-bottom: 0;
  }

  .msg.md :global(h4),
  .msg.md :global(h5),
  .msg.md :global(h6) {
    margin: 8px 0 4px;
    font-size: 13px;
  }

  .msg.md :global(ul),
  .msg.md :global(ol) {
    padding-left: 20px;
  }

  .msg.md :global(li.nested) {
    margin-left: 16px;
    list-style-type: circle;
  }

  .msg.md :global(code) {
    font-family: var(--font-mono, monospace);
    font-size: 12px;
    background: var(--bg-inset);
    border-radius: 3px;
    padding: 0 3px;
  }

  .msg.md :global(pre) {
    background: var(--bg-inset);
    border-radius: var(--r-sm);
    padding: 6px 8px;
    overflow-x: auto;
    white-space: pre;
  }

  .msg.md :global(pre code) {
    background: none;
    padding: 0;
  }

  .msg.md :global(blockquote) {
    border-left: 3px solid var(--border);
    padding-left: 8px;
    color: var(--text-dim);
  }

  .msg.md :global(.md-table) {
    overflow-x: auto;
  }

  .msg.md :global(table) {
    border-collapse: collapse;
    font-size: 12px;
  }

  .msg.md :global(th),
  .msg.md :global(td) {
    border: 1px solid var(--border);
    padding: 2px 6px;
    text-align: left;
  }

  .msg.md :global(hr) {
    border: none;
    border-top: 1px solid var(--border);
    margin: 6px 0;
  }

  .msg.tool {
    align-self: flex-start;
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-size: 11px;
    color: var(--ai);
    background: color-mix(in srgb, var(--ai) 12%, transparent);
    padding: 3px 9px;
  }

  .msg.turn {
    align-self: flex-start;
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 11px;
    color: var(--text-dim);
    padding: 2px 4px;
  }

  .msg.notice {
    align-self: center;
    color: var(--text-dim);
    font-size: 11px;
    background: none;
    padding: 2px 8px;
  }

  .msg.error {
    align-self: stretch;
    background: var(--danger-bg);
    color: var(--danger-text);
    font-size: 12px;
  }

  .msg.thinking {
    align-self: flex-start;
    padding: 8px 12px;
  }

  .dots::after {
    content: "…";
    color: var(--ai);
    animation: blink 1.2s infinite;
    font-weight: 700;
  }

  @keyframes blink {
    0%,
    100% {
      opacity: 0.3;
    }
    50% {
      opacity: 1;
    }
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 6px 10px 0;
  }

  .chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 22px;
    padding: 0 3px 0 8px;
    border-radius: 11px;
    border: 1px solid var(--accent-dim);
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    color: var(--accent);
    font-size: var(--fs-xs);
    max-width: 100%;
  }

  .chip-x {
    display: inline-flex;
    align-items: center;
    padding: 2px;
    border: none;
    border-radius: 50%;
    background: none;
    color: var(--accent);
    opacity: 0.75;
  }

  .chip-x:hover:not(:disabled) {
    opacity: 1;
    border: none;
    background: color-mix(in srgb, var(--accent) 20%, transparent);
  }

  .input-row {
    display: flex;
    gap: 6px;
    padding: 8px 10px 10px;
    border-top: 1px solid var(--border);
  }

  textarea {
    flex: 1;
    resize: none;
    line-height: 1.4;
    box-sizing: border-box;
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 6px 8px;
    font-family: inherit;
    font-size: 13px;
  }

  textarea:focus {
    outline: none;
    border-color: var(--accent-dim);
  }

  .send,
  .stop {
    align-self: flex-end;
    padding: 6px 14px;
  }

  .stop {
    border-color: #8a4a54;
    color: var(--danger-text);
  }
</style>
