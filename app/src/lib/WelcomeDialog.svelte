<script lang="ts">
  // はじめの確認: 初めて起動したときに、音が出るか・AI のチャットが使えるか・SoundFont があるかを確かめ、
  // 足りないものをその場で用意できるようにする(以前は空の曲が開くだけで、CLI が無いことは送信して初めて分かった)。
  // 設定の「表示」からいつでも開き直せる。
  import { onMount } from "svelte";
  import { open as pickFile } from "@tauri-apps/plugin-dialog";
  import Icon from "./Icon.svelte";
  import * as api from "./api";
  import { tr } from "./i18n.svelte";
  import { chatStatus } from "./aiStatus.svelte";
  import { selectionStore } from "./selection.svelte";
  import { openSettings, saveSettings, settings, welcomeUi } from "./settings.svelte";
  import { showError, showToast } from "./toast.svelte";

  let status = $state<api.SetupStatus | null>(null);
  let sfProgress = $state<{ got: number; total: number } | null>(null);
  let busy = $state(false);

  async function check() {
    try {
      status = await api.setupStatus();
    } catch (e) {
      showError(tr("状態を確かめられませんでした", "Couldn't check the status"), e);
    }
  }

  onMount(() => {
    check();
  });

  function close() {
    settings.welcomeDone = true;
    saveSettings();
    welcomeUi.open = false;
  }

  const mb = (b: number) => `${Math.round(b / 1e6)}MB`;

  async function downloadSf() {
    if (busy) return;
    busy = true;
    sfProgress = { got: 0, total: status?.soundfont.download_bytes ?? 1 };
    const un = await api.onSoundFontDownload((p) => (sfProgress = p)).catch(() => undefined);
    try {
      await api.downloadSoundFont();
      showToast(
        "ok",
        tr(
          "GM 音源の SoundFont を入れました。音源の選択で「SoundFont」から使えます",
          "Installed the GM SoundFont. Use it from \"SoundFont\" in the instrument picker",
        ),
      );
    } catch (e) {
      showError(tr("SoundFont を取得できませんでした", "Couldn't download the SoundFont"), e);
    } finally {
      un?.();
      sfProgress = null;
      busy = false;
      check();
    }
  }

  async function addSf() {
    const file = await pickFile({ title: tr("SoundFont(.sf2)をライブラリに追加", "Add a SoundFont (.sf2) to the library"), filters: [{ name: "SoundFont", extensions: ["sf2"] }] });
    if (typeof file !== "string") return;
    try {
      await api.addSoundfont(file);
      check();
    } catch (e) {
      showError(tr("SoundFont を追加できませんでした", "Couldn't add the SoundFont"), e);
    }
  }

  async function openDemo() {
    if (busy) return;
    busy = true;
    try {
      const r = await api.openDemoSong();
      // プロジェクトの切り替えと同じ後始末(会話の表示・選んだ範囲はプロジェクトごと)
      chatStatus.epoch += 1;
      selectionStore.range = null;
      showToast(
        "ok",
        tr(
          `デモ曲「${r.title}」を開きました(写しなので自由に直せます)。スペースで再生`,
          `Opened the demo song "${r.title}" (it's a copy, so feel free to edit it). Press Space to play`,
        ),
      );
      close();
    } catch (e) {
      showError(tr("デモ曲を開けませんでした", "Couldn't open the demo song"), e);
    } finally {
      busy = false;
    }
  }

  const aiReady = $derived(!!status && (!!status.claude || !!status.codex));
</script>

<div class="backdrop" role="presentation" onclick={close}></div>
<div class="panel" role="dialog" aria-label={tr("はじめの確認", "Setup check")}>
  <div class="head">
    <h2><Icon name="sparkles" />{tr("Glaux へようこそ", "Welcome to Glaux")}</h2>
    <button class="btn sm icon ghost" onclick={close} title={tr("閉じる", "Close")} aria-label={tr("閉じる", "Close")}><Icon name="x" /></button>
  </div>
  <p class="lead">
    {tr(
      "使い始める前に、この PC で必要なものがそろっているかを確かめます。足りないものはここで用意できます。",
      "Before you start, let's check that this PC has everything needed. You can set up anything missing right here.",
    )}
  </p>

  {#if !status}
    <div class="note">{tr("確かめています…", "Checking…")}</div>
  {:else}
    <div class="item">
      <span class="mark" class:ok={!!status.audio_output} class:ng={!status.audio_output}
        ><Icon name={status.audio_output ? "check" : "triangle-alert"} size={14} /></span
      >
      <div class="body">
        <div class="title">{tr("音の出力", "Audio output")}</div>
        {#if status.audio_output}
          <div class="note">{tr(`${status.audio_output} から鳴ります`, `Playing through ${status.audio_output}`)}</div>
        {:else}
          <div class="note">
            {tr(
              "オーディオの出力を開けませんでした。スピーカー・ヘッドホンをつないで、設定の「オーディオ」で選び直してください",
              "Couldn't open the audio output. Connect speakers or headphones, then choose it again in Settings > Audio",
            )}
          </div>
          <button class="btn sm" onclick={() => openSettings("audio")}>{tr("オーディオの設定を開く", "Open audio settings")}</button>
        {/if}
      </div>
    </div>

    <div class="item">
      <span class="mark" class:ok={aiReady} class:warn={!aiReady}
        ><Icon name={aiReady ? "check" : "info"} size={14} /></span
      >
      <div class="body">
        <div class="title">{tr("AI のチャット", "AI chat")}</div>
        <div class="cli">
          <span class:found={!!status.claude}>Claude Code: {status.claude ? tr("見つかりました", "Found") : tr("見つかりません", "Not found")}</span>
          <span class:found={!!status.codex}
            >{tr("Codex CLI(GPT)", "Codex CLI (GPT)")}: {status.codex ? tr("見つかりました", "Found") : tr("見つかりません", "Not found")}</span
          >
        </div>
        {#if !aiReady}
          <div class="note">
            {tr(
              "チャットには、どちらかをこの PC に入れてログインしておく必要があります(入れた後は Glaux を起動し直す)。 AI 無しでも、打ち込み・ミックス・書き出しはすべて使えます。",
              "To chat, install one of these on this PC and sign in (restart Glaux afterwards). Without AI, note entry, mixing, and export all still work.",
            )}
          </div>
          <ul class="how">
            <li>Claude Code: <code>npm i -g @anthropic-ai/claude-code</code> → {tr("", "sign in with ")}<code>claude</code>{tr(" でログイン", "")}</li>
            <li>Codex CLI: <code>npm i -g @openai/codex</code> → <code>codex login</code></li>
          </ul>
        {:else}
          <div class="note">
            {tr(
              "右のチャット欄で「4 小節のベースを作って」のように頼めます。相手はチャット欄の左上で切り替えます",
              "In the chat panel on the right, ask things like \"Make a 4-bar bassline\". Switch the AI at the top left of the chat panel",
            )}
          </div>
        {/if}
      </div>
    </div>

    <div class="item">
      <span class="mark" class:ok={status.soundfont.files.length > 0} class:warn={status.soundfont.files.length === 0}
        ><Icon name={status.soundfont.files.length > 0 ? "check" : "info"} size={14} /></span
      >
      <div class="body">
        <div class="title">{tr("SoundFont(ピアノ・ストリングス・ブラスなどの GM 音源一式)", "SoundFont (a full GM sound set: piano, strings, brass, and more)")}</div>
        {#if status.soundfont.files.length > 0}
          <div class="note">{tr(`${status.soundfont.files.join("、")} が使えます`, `Available: ${status.soundfont.files.join(", ")}`)}</div>
        {:else}
          <div class="note">
            {tr(
              "無くても内蔵のシンセ・ドラムで曲を作れます。生楽器の音が欲しいときは入れてください (GeneralUser GS。S. Christian Collins 作、音楽制作に私用・商用とも自由に使えます)",
              "You can make songs with the built-in synths and drums without it. Install it if you want acoustic instrument sounds (GeneralUser GS by S. Christian Collins, free for personal and commercial music production)",
            )}
          </div>
        {/if}
        {#if sfProgress}
          <div class="progress"><i style="width:{Math.min(100, (sfProgress.got / Math.max(1, sfProgress.total)) * 100)}%"></i></div>
          <div class="note">{mb(sfProgress.got)} / {mb(sfProgress.total)}</div>
        {:else if !status.soundfont.files.includes(status.soundfont.download_file)}
          <div class="row">
            <button class="btn sm" onclick={downloadSf} disabled={busy}
              ><Icon name="download" />{tr(`GM 音源を取得(${mb(status.soundfont.download_bytes)})`, `Download GM sounds (${mb(status.soundfont.download_bytes)})`)}</button
            >
            <button class="btn sm" onclick={addSf} disabled={busy}>{tr("手持ちの .sf2 を追加…", "Add your own .sf2…")}</button>
          </div>
        {/if}
      </div>
    </div>

    <div class="item">
      <span class="mark"><Icon name="music" size={14} /></span>
      <div class="body">
        <div class="title">{tr("デモ曲", "Demo song")}</div>
        <div class="note">
          {tr(
            "内蔵の音源だけで作ったドラムンベース(CyberNeon、174 BPM)を、写しとして開きます。AI に「もっと明るく」などと頼んで試せます",
            "Opens a copy of a drum and bass track (CyberNeon, 174 BPM) made only with built-in instruments. Try asking the AI things like \"make it brighter\"",
          )}
        </div>
        <div class="row">
          <button class="btn sm" onclick={openDemo} disabled={busy}><Icon name="folder-open" />{tr("デモ曲を開く", "Open demo song")}</button>
        </div>
      </div>
    </div>
  {/if}

  <div class="foot">
    <button class="btn sm ghost" onclick={check} disabled={busy}><Icon name="refresh-cw" />{tr("確かめ直す", "Check again")}</button>
    <button class="go" onclick={close}>{tr("始める", "Get started")}</button>
  </div>
  <div class="note small">{tr("この画面は設定の「表示」の「はじめの確認」からいつでも開けます", "You can open this screen anytime from Settings > Display > Setup check")}</div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 39;
    background: rgba(0, 0, 0, 0.5);
  }

  .panel {
    position: fixed;
    z-index: 40;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: 520px;
    max-width: calc(100vw - 32px);
    max-height: calc(100vh - 40px);
    overflow-y: auto;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.5);
    padding: 16px 18px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .head {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  h2 {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0;
    font-size: 16px;
  }

  h2 :global(.icon) {
    color: var(--accent);
  }

  .lead {
    margin: 0;
    color: var(--text-dim);
    font-size: 12px;
    line-height: 1.6;
  }

  .item {
    display: flex;
    gap: 10px;
    padding: 10px 0;
    border-top: 1px solid var(--border);
  }

  .mark {
    flex-shrink: 0;
    width: 22px;
    height: 22px;
    border-radius: 50%;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--text-dim);
    background: var(--bg-inset);
  }

  .mark.ok {
    color: var(--accent);
  }

  .mark.warn {
    color: var(--warn);
  }

  .mark.ng {
    color: var(--danger-text);
    background: var(--danger-bg);
  }

  .body {
    display: flex;
    flex-direction: column;
    gap: 5px;
    min-width: 0;
    flex: 1;
  }

  .title {
    font-size: 13px;
    font-weight: 600;
  }

  .note {
    font-size: 12px;
    color: var(--text-dim);
    line-height: 1.6;
  }

  .note.small {
    font-size: var(--fs-xs);
    text-align: right;
  }

  .cli {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 14px;
    font-size: 12px;
    color: var(--text-dim);
  }

  .cli .found {
    color: var(--text);
  }

  .how {
    margin: 0;
    padding-left: 18px;
    font-size: 12px;
    color: var(--text-dim);
    line-height: 1.8;
  }

  code {
    font-family: var(--font-mono, monospace);
    font-size: 11px;
    background: var(--bg-inset);
    border-radius: 3px;
    padding: 1px 4px;
    user-select: text;
  }

  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .progress {
    height: 6px;
    border-radius: 3px;
    background: var(--bg-inset);
    overflow: hidden;
  }

  .progress i {
    display: block;
    height: 100%;
    background: var(--accent);
  }

  .foot {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding-top: 10px;
    border-top: 1px solid var(--border);
  }

  .go {
    padding: 6px 22px;
  }
</style>
