<script lang="ts">
  import { onMount, tick } from "svelte";
  import * as api from "./api";
  import { chatStatus } from "./aiStatus.svelte";
  import { toolShort } from "./toolLabels";
  import { clearAiHighlight, setAiHighlight } from "./aiHighlight.svelte";
  import { showError, showToast } from "./toast.svelte";
  import { playDoneChime, playErrorChime, saveSettings, settings } from "./settings.svelte";

  // チャットの相手。Claude = Claude Code(claude)、GPT = Codex CLI(codex)。どちらもホストでログイン済みのものを使う
  const PROVIDERS = [
    { value: "claude", label: "Claude" },
    { value: "codex", label: "GPT" },
  ] as const;

  // モデルの選択肢("" は各 CLI の既定)。Claude は claude --model のエイリアス、GPT は codex -m のモデル名
  const MODELS = {
    claude: [
      { value: "", label: "既定" },
      { value: "opus", label: "Opus" },
      { value: "sonnet", label: "Sonnet" },
      { value: "haiku", label: "Haiku" },
    ],
    codex: [
      { value: "", label: "既定" },
      { value: "gpt-6-astra", label: "GPT-6 Astra" },
    ],
  };
  const MODEL_EXAMPLES = {
    claude: "例: claude-opus-5-5、claude-sonnet-5",
    codex: "例: gpt-6-astra",
  };
  const provider = $derived(settings.chatProvider);
  const currentModel = $derived(provider === "codex" ? settings.chatCodexModel : settings.chatModel);
  const isCustomModel = $derived(!MODELS[provider].some((m) => m.value === currentModel));

  function setModel(v: string) {
    if (provider === "codex") settings.chatCodexModel = v;
    else settings.chatModel = v;
  }

  function pickProvider(e: Event) {
    const v = (e.currentTarget as HTMLSelectElement).value;
    settings.chatProvider = v === "codex" ? "codex" : "claude";
    saveSettings();
  }

  function pickModel(e: Event) {
    const v = (e.currentTarget as HTMLSelectElement).value;
    if (v === "__custom") {
      const name = window.prompt(`モデル名(${MODEL_EXAMPLES[provider]})`, currentModel);
      if (name === null) {
        (e.currentTarget as HTMLSelectElement).value = isCustomModel ? "__current" : currentModel;
        return;
      }
      setModel(name.trim());
    } else {
      setModel(v);
    }
    saveSettings();
  }
  import { MASTER_FOCUS_ID, pianoRollStore, selectionStore, soundDesignStore } from "./selection.svelte";

  interface Msg {
    role: "user" | "assistant" | "tool" | "notice" | "error" | "turn";
    text: string;
    /** role "turn": ターンの開始前の最後の履歴エントリ(取り消しの起点)と、取り消したか */
    since?: string | null;
    reverted?: boolean;
  }

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

  // プロジェクト切り替えで会話表示をクリアする(会話自体はプロジェクトごとに保存されている)
  $effect(() => {
    void chatStatus.epoch;
    messages = [];
  });

  async function scrollToBottom() {
    await tick();
    scroller?.scrollTo({ top: scroller.scrollHeight });
  }

  function push(msg: Msg) {
    messages.push(msg);
    scrollToBottom();
  }

  async function send() {
    const prompt = input.trim();
    if (!prompt || chatStatus.running) return;

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
      shown = `〔🎛 マスター〕 ${shown}`;
    } else if (sd) {
      prefix +=
        `【音作り中のトラック(ユーザーが音作りビューで開いている)】「${sd.trackName}」(${sd.trackId})。` +
        `音色・エフェクトに関する指示は、特に指定がなければこのトラックが対象です。\n`;
      shown = `〔🎛 ${sd.trackName}〕 ${shown}`;
    }
    const fullPrompt = prefix ? `${prefix}\n${prompt}` : prompt;
    push({ role: "user", text: shown });

    input = "";
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
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      send();
    }
  }

  async function cancel() {
    await api.cancelChat();
  }

  async function newConversation() {
    if (chatStatus.running) return;
    await api.resetChat();
    messages = [];
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
            finishTurn();
            break;
          case "notice":
            push({ role: "notice", text: ev.text });
            break;
          case "error":
            chatStatus.running = false;
            push({ role: "error", text: ev.message });
            playErrorChime();
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
    <h2>AI に指示</h2>
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
      <button class="small" onclick={newConversation} disabled={chatStatus.running} title="会話の文脈をリセットする">
        新しい会話
      </button>
    </div>
  </div>

  <div class="messages" bind:this={scroller}>
    {#if messages.length === 0}
      <div class="hint">
        例:「4小節のベースラインを作って」「もっと音を明るくして」「さっきの編集を取り消して」
      </div>
    {/if}
    {#each messages as m}
      {#if m.role === "tool"}
        <div class="msg tool">⚙ {m.text}</div>
      {:else if m.role === "turn"}
        <div class="msg turn">
          <span>{m.text}{m.reverted ? "(取り消し済み)" : ""}</span>
          {#if !m.reverted}
            <button
              class="small"
              onclick={() => revertTurnAt(m)}
              title="このターンで AI が行った編集をまとめて取り消す(後から人間が行った編集は残す)"
              >↩ 取り消す</button
            >
          {/if}
        </div>
      {:else}
        <div class="msg {m.role}">{m.text}</div>
      {/if}
    {/each}
    {#if chatStatus.running}
      <div class="msg thinking"><span class="dots"></span></div>
    {/if}
  </div>

  {#if pianoRollStore.focus}
    <div class="range-chip">
      <span>対象クリップ: {pianoRollStore.focus.clipName}(ピアノロールで編集中)</span>
    </div>
  {/if}
  {#if soundDesignStore.focus}
    <div class="range-chip">
      <span
        >🎛 音作り中: {soundDesignStore.focus.trackName}{soundDesignStore.focus.trackId === MASTER_FOCUS_ID
          ? "(エフェクトの指示はマスターへ)"
          : "(音色の指示はこのトラックへ)"}</span
      >
    </div>
  {/if}
  {#if selectionStore.range}
    <div class="range-chip">
      <span>
        対象: 小節 {selectionStore.range.startBar + 1}〜{selectionStore.range.endBar + 1}
        (この範囲に限定して指示されます)
      </span>
      <button class="chip-clear" onclick={() => (selectionStore.range = null)} title="範囲指定を解除">
        ✕
      </button>
    </div>
  {/if}

  <div class="input-row">
    <textarea
      bind:this={inputEl}
      rows="2"
      placeholder="AI への指示を入力(Enter で送信 / Shift+Enter で改行)"
      bind:value={input}
      onkeydown={onKeydown}
      disabled={chatStatus.running}
    ></textarea>
    {#if chatStatus.running}
      <button class="stop" onclick={cancel} title="実行中の指示を中断する">停止</button>
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
    font-size: 11px;
    max-width: 130px;
  }

  h2 {
    font-size: 13px;
    margin: 0;
    color: var(--text-dim);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  button.small {
    font-size: 11px;
    padding: 2px 8px;
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

  .msg.tool {
    align-self: flex-start;
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

  .range-chip {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin: 0 10px;
    padding: 4px 10px;
    border-radius: 6px 6px 0 0;
    background: color-mix(in srgb, var(--accent) 15%, transparent);
    color: var(--accent);
    font-size: 11px;
  }

  .chip-clear {
    padding: 0 6px;
    font-size: 11px;
    border: none;
    background: none;
    color: var(--accent);
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
