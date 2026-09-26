<script lang="ts">
  import Icon from "./Icon.svelte";
  // 書き出しの画面。以前は 48kHz / 16bit / 曲全体 / プロジェクトの export/ 固定で、選べるものが無かった。
  import { open as pickDir, save as pickFile } from "@tauri-apps/plugin-dialog";
  import * as api from "./api";
  import { selectionStore } from "./selection.svelte";
  import { showError, showToast } from "./toast.svelte";

  let {
    onClose,
    loop,
  }: {
    onClose: () => void;
    /** ループ区間 [開始 tick, 終了 tick](無ければ null) */
    loop: [number, number] | null;
  } = $props();

  let target = $state<"mix" | "stems" | "midi">("mix");
  let range = $state<"all" | "loop" | "selection">("all");
  let sampleRate = $state(48000);
  let bits = $state(16);
  let noiseShaping = $state(true);
  let loudness = $state<string>("none");
  let path = $state<string | null>(null);
  let busy = $state(false);
  let result = $state<string | null>(null);

  // 対象を変えたら保存先は既定に戻す(WAV・フォルダ・.mid で選ぶものが違う)
  $effect(() => {
    void target;
    path = null;
    result = null;
  });

  const LOUDNESS = [
    { value: "none", label: "そのまま" },
    { value: "-14", label: "-14 LUFS(配信: YouTube・Spotify など)" },
    { value: "-16", label: "-16 LUFS(Apple Music・ポッドキャスト)" },
    { value: "-23", label: "-23 LUFS(放送)" },
  ];

  async function choosePath() {
    try {
      const p =
        target === "stems"
          ? await pickDir({ directory: true, title: "トラックごとの WAV を書き出すフォルダ" })
          : target === "midi"
            ? await pickFile({ title: "書き出す MIDI ファイル", filters: [{ name: "MIDI", extensions: ["mid"] }] })
            : await pickFile({ title: "書き出す WAV ファイル", filters: [{ name: "WAV", extensions: ["wav"] }] });
      if (typeof p === "string") path = p;
    } catch (e) {
      showError("保存先を選べませんでした", e);
    }
  }

  async function run() {
    if (busy) return;
    if (target === "midi") {
      busy = true;
      result = null;
      try {
        const r = await api.exportMidi(path ?? undefined);
        result = `MIDI に書き出しました(トラック ${r.tracks} 本): ${r.path}`;
        showToast("ok", result);
      } catch (e) {
        showError("書き出せませんでした", e);
      } finally {
        busy = false;
      }
      return;
    }
    const request: api.ExportRequest = {
      path: path ?? undefined,
      sample_rate: sampleRate,
      bits,
      noise_shaping: bits === 16 ? noiseShaping : undefined,
      stems: target === "stems",
      loudness_lufs: target === "mix" && loudness !== "none" ? Number(loudness) : undefined,
    };
    if (range === "loop" && loop) {
      request.start_tick = loop[0];
      request.end_tick = loop[1];
    } else if (range === "selection" && selectionStore.range) {
      request.start_tick = selectionStore.range.startTick;
      request.end_tick = selectionStore.range.endTick;
    }
    busy = true;
    result = null;
    try {
      const r = await api.exportAudio(request);
      if (r.stems) {
        result = `トラック ${r.stems.length} 本を書き出しました: ${r.folder}`;
      } else {
        const lufs = Number.isFinite(r.lufs) ? `${r.lufs} LUFS / ` : "";
        const gain = r.gain_db ? ` / 音量 ${r.gain_db > 0 ? "+" : ""}${r.gain_db} dB` : "";
        const lim = r.limiter_db ? ` / リミッタ最大 -${r.limiter_db} dB` : "";
        const sp = r.streaming?.find((s) => s.service === "Spotify");
        const spot = sp && sp.gain_db < -0.5 ? `。Spotify では ${-sp.gain_db} dB 下げて再生される見込み` : "";
        result = `書き出しました(${r.seconds?.toFixed(1)} 秒、${lufs}True Peak ${r.true_peak_db ?? r.peak_db} dBTP${gain}${lim}${spot}): ${r.path}`;
      }
      showToast("ok", result);
    } catch (e) {
      showError("書き出せませんでした", e);
    } finally {
      busy = false;
    }
  }
</script>

<div class="backdrop" role="presentation" onclick={onClose}></div>
<div class="panel" role="dialog" aria-label="書き出し">
  <div class="head">
    <h2><Icon name="download" />書き出し</h2>
    <button class="btn sm icon ghost" onclick={onClose} title="閉じる(Esc)" aria-label="閉じる"><Icon name="x" /></button>
  </div>

  <div class="section">
    <div class="section-title">対象</div>
    <label class="row"><input type="radio" bind:group={target} value="mix" /> 曲全体(ミックス)</label>
    <label class="row"
      ><input type="radio" bind:group={target} value="stems" /> トラックごと(ステム。マスターのエフェクトは通さない)</label
    >
    <label class="row"
      ><input type="radio" bind:group={target} value="midi" /> MIDI ファイル(ノート・テンポ・拍子・マーカー。ループは展開)</label
    >
  </div>

  {#if target !== "midi"}
    <div class="section">
      <div class="section-title">範囲</div>
      <label class="row"><input type="radio" bind:group={range} value="all" /> 曲全体</label>
      <label class="row" class:off={!loop}
        ><input type="radio" bind:group={range} value="loop" disabled={!loop} /> ループ区間</label
      >
      <label class="row" class:off={!selectionStore.range}
        ><input type="radio" bind:group={range} value="selection" disabled={!selectionStore.range} />
        選んだ小節{selectionStore.range
          ? `(${selectionStore.range.startBar + 1}〜${selectionStore.range.endBar + 1} 小節)`
          : "(ルーラーをドラッグして選ぶ)"}</label
      >
    </div>

    <div class="section">
      <div class="section-title">形式</div>
      <div class="row">
        <select bind:value={sampleRate} aria-label="サンプルレート">
          <option value={48000}>48 kHz</option>
          <option value={44100}>44.1 kHz(CD・配信)</option>
        </select>
        <select bind:value={bits} aria-label="ビット数">
          <option value={16}>16 bit</option>
          <option value={24}>24 bit</option>
          <option value={32}>32 bit 浮動小数</option>
        </select>
      </div>
      {#if bits === 16}
        <label
          class="check"
          title="16bit に丸めるときの雑音を、耳が敏感な 2〜5kHz から聞こえにくい高域へ寄せる(聞こえ方で約 20dB 静か)。書き出した後でさらに加工・変換するなら外す"
          ><input type="checkbox" bind:checked={noiseShaping} /> ノイズシェーピング</label
        >
      {/if}
    </div>

  {/if}

  {#if target === "mix"}
    <div class="section">
      <div class="section-title">音量</div>
      <select bind:value={loudness} aria-label="音量の目標">
        {#each LOUDNESS as l (l.value)}
          <option value={l.value}>{l.label}</option>
        {/each}
      </select>
      {#if loudness !== "none"}
        <div class="note">曲全体の音量をこの値に合わせ、True Peak(サンプルの間の山も含むピーク)が -1 dBTP を超える所はリミッタで抑えます</div>
      {/if}
    </div>
  {/if}

  <div class="section">
    <div class="section-title">保存先</div>
    <div class="row">
      <code class="path" title={path ?? ""}>{path ?? "プロジェクトの export フォルダ(日時付きの名前)"}</code>
      <button onclick={choosePath}>選ぶ…</button>
      {#if path}<button class="btn sm icon ghost" onclick={() => (path = null)} title="既定に戻す" aria-label="既定に戻す"
          ><Icon name="x" /></button
        >{/if}
    </div>
  </div>

  <button class="go" onclick={run} disabled={busy}>{busy ? "書き出し中…" : "書き出す"}</button>
  {#if result}<div class="note result">{result}</div>{/if}
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 29;
    background: rgba(0, 0, 0, 0.45);
  }

  .panel {
    position: fixed;
    z-index: 30;
    top: 60px;
    right: 16px;
    width: 380px;
    max-width: calc(100vw - 32px);
    max-height: calc(100vh - 80px);
    overflow-y: auto;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.5);
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 14px;
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
    font-size: var(--fs-lg);
  }

  h2 :global(.icon) {
    color: var(--accent);
  }

  .section {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .section-title {
    font-size: 11px;
    color: var(--text-dim);
    letter-spacing: 0.05em;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
  }

  .row.off {
    color: var(--text-dim);
  }

  .path {
    flex: 1;
    min-width: 0;
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-dim);
  }

  .note {
    font-size: 11px;
    color: var(--text-dim);
    line-height: 1.5;
  }

  .result {
    word-break: break-all;
  }

  .go {
    padding: 6px 12px;
    border-color: var(--accent-dim);
    color: var(--accent);
  }

  .check {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 6px;
    font-size: var(--fs-sm);
    color: var(--text-dim);
    cursor: pointer;
  }
</style>
