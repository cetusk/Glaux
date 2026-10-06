<script lang="ts">
  // ステータスバー: プロジェクトフォルダ・音の処理の負荷・デバイス・MCP・版数(押すと設定・コピー)
  import Icon from "./Icon.svelte";
  import { APP_VERSION, APP_VERSION_DETAIL } from "./appVersion";
  import { tr } from "./i18n.svelte";
  import { openSettings } from "./settings.svelte";
  import { transportStore } from "./transport.svelte";
  import type { AppInfo } from "./types";

  let {
    info,
    audioDev,
    projectVersion,
  }: {
    info: AppInfo | null;
    /// 使用中のオーディオデバイス(取得できない環境では null)
    audioDev: { output: string | null; input: string | null; rate: number; midi: string | null } | null;
    projectVersion: number;
  } = $props();

  const transport = $derived(transportStore.state);
  let mcpCopied = $state(false);

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

  /// 窓口のポートが既定(41920)と違うとき(Glaux を複数起動したとき)は、表示にポートを添える
  function mcpLabel(url: string): string {
    const port = url.match(/:(\d+)\//)?.[1];
    return port && port !== "41920" ? `MCP :${port}` : "MCP";
  }

  async function copyMcpUrl() {
    if (!info) return;
    await navigator.clipboard.writeText(
      `claude mcp add --transport http glaux ${info.mcp_url}`,
    );
    mcpCopied = true;
    setTimeout(() => (mcpCopied = false), 1500);
  }
</script>

<!-- ステータスバー: 負荷・デバイス・MCP・版数(押すと設定・コピー) -->
<footer>
  {#if info}
    <code class="path" title={tr("プロジェクトフォルダ", "Project folder")}>{info.project_dir}</code>
  {/if}
  <div class="st">
    {#if transport.playing || dspWarn}
      <span
        class="it"
        class:warn={dspWarn}
        title={tr(
          `音の処理の負荷(この再生の開始から)\n平均 ${dspAvg.toFixed(0)}% / 直近 3 秒の最大 ${dspMax.toFixed(0)}%\n処理落ち(Glaux の計算が間に合わない): ${dspCounts.overruns} 回\n呼び出し遅延(他の処理に CPU を奪われた): ${dspCounts.late} 回\nOS が知らせた音切れ: ${dspCounts.xruns} 回\n再生データの差し替え(編集で音が切り直される): ${dspCounts.swaps} 回${transport.dsp?.realtime_denied ? "\nOS がリアルタイム優先度を認めていません(途切れやすくなります)" : ""}${dspDrops > 0 ? "\n途切れるときは、設定 → オーディオでバッファを大きくしてください" : ""}`,
          `Audio processing load (since playback started)\nAverage ${dspAvg.toFixed(0)}% / max over last 3 s ${dspMax.toFixed(0)}%\nOverruns (Glaux could not finish in time): ${dspCounts.overruns}\nLate callbacks (CPU taken by other processes): ${dspCounts.late}\nDropouts reported by the OS: ${dspCounts.xruns}\nPlayback data swaps (sound restarts after edits): ${dspCounts.swaps}${transport.dsp?.realtime_denied ? "\nThe OS denied real-time priority (dropouts are more likely)" : ""}${dspDrops > 0 ? "\nIf audio drops out, increase the buffer size in Settings → Audio" : ""}`,
        )}
        ><Icon name="cpu" size={12} />{dspAvg.toFixed(0)}%{#if dspDrops > 0}
          <Icon name="triangle-alert" size={12} />{dspDrops}{/if}</span
      >
    {/if}
    {#if audioDev}
      <button class="it" onclick={() => openSettings("audio")} title={tr("出力デバイス(クリックで設定)", "Output device (click for settings)")}
        ><Icon name="speaker" size={12} />{audioDev.output ?? tr("なし", "None")}{audioDev.rate
          ? ` · ${(audioDev.rate / 1000).toFixed(1)} kHz`
          : ""}</button
      >
      <button class="it" onclick={() => openSettings("audio")} title={tr("入力デバイス(クリックで設定)", "Input device (click for settings)")}
        ><Icon name="mic" size={12} />{audioDev.input ?? tr("なし", "None")}</button
      >
      <button class="it" onclick={() => openSettings("midi")} title={tr("MIDI 入力(クリックで設定)", "MIDI input (click for settings)")}
        ><Icon name="keyboard-music" size={12} />{audioDev.midi ?? tr("なし", "None")}</button
      >
    {/if}
    {#if info}
      <button class="it" onclick={copyMcpUrl} title={tr(
          `MCP サーバー ${info.mcp_url}\nクリックで Claude Code への登録コマンドをコピー`,
          `MCP server ${info.mcp_url}\nClick to copy the command to register it with Claude Code`,
        )}
        ><Icon name={mcpCopied ? "check" : "link"} size={12} />{mcpCopied ? tr("コピーしました", "Copied") : mcpLabel(info.mcp_url)}</button
      >
    {/if}
    <span class="it" title={tr("この曲の編集の回数(編集・取り消し・やり直しのたびに増える)", "Edit count for this song (increases with every edit, undo and redo)")}
      ><Icon name="history" size={12} />{tr("編集", "Edits")} {projectVersion}</span
    >
    <button class="it" onclick={() => openSettings("about")} title={tr(`${APP_VERSION_DETAIL}(クリックで「Glaux について」)`, `${APP_VERSION_DETAIL} (click for "About Glaux")`)}
      >v{APP_VERSION}</button
    >
  </div>
</footer>

<style>
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
</style>
