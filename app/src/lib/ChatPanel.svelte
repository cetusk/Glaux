<script lang="ts">
  import { onMount, tick } from "svelte";
  import * as api from "./api";
  import { chatStatus } from "./aiStatus.svelte";
  import { pianoRollStore, selectionStore } from "./selection.svelte";

  interface Msg {
    role: "user" | "assistant" | "tool" | "notice" | "error";
    text: string;
  }

  const toolLabels: Record<string, string> = {
    get_project: "プロジェクトを確認",
    get_history: "履歴を確認",
    apply_commands: "編集を適用",
    undo: "取り消し",
    redo: "やり直し",
    checkpoint: "チェックポイント作成",
    revert_to: "チェックポイントへ巻き戻し",
  };

  let messages = $state<Msg[]>([]);
  let input = $state("");
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
    const fullPrompt = prefix ? `${prefix}\n${prompt}` : prompt;
    push({ role: "user", text: shown });

    input = "";
    chatStatus.running = true;
    try {
      await api.sendChat(fullPrompt);
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
            push({ role: "tool", text: toolLabels[ev.name] ?? ev.name });
            break;
          case "result":
            chatStatus.running = false;
            // 成功時の最終テキストは assistant_text で受信済みなので出さない
            if (!ev.ok && ev.text) {
              push({ role: "error", text: ev.text });
            }
            break;
          case "notice":
            push({ role: "notice", text: ev.text });
            break;
          case "error":
            chatStatus.running = false;
            push({ role: "error", text: ev.message });
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
    <button class="small" onclick={newConversation} disabled={chatStatus.running} title="会話の文脈をリセットする">
      新しい会話
    </button>
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

  .msg.notice {
    align-self: center;
    color: var(--text-dim);
    font-size: 11px;
    background: none;
    padding: 2px 8px;
  }

  .msg.error {
    align-self: stretch;
    background: #46242c;
    color: #ffb4c0;
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
    resize: vertical;
    min-height: 40px;
    max-height: 45vh;
    resize: none;
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
    color: #ffb4c0;
  }
</style>
