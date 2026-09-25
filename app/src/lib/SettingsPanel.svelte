<script lang="ts">
  import Icon from "./Icon.svelte";
  import {
    ACCENT_PRESETS,
    applyTheme,
    CHAT_MODEL_EXAMPLES,
    CHAT_MODELS,
    CHAT_PROVIDERS,
    playDoneChime,
    saveSettings,
    setChatModel,
    settings,
    settingsUi,
    type SettingsTab,
  } from "./settings.svelte";
  import type { IconName } from "./icons";

  import { onMount, untrack } from "svelte";
  import * as api from "./api";
  import { requestFastPolling, transportStore } from "./transport.svelte";

  let { onClose }: { onClose: () => void } = $props();

  // ---- オーディオデバイス ----
  let devices = $state<api.AudioDevices | null>(null);
  let deviceMsg = $state<string | null>(null);

  async function loadDevices() {
    try {
      devices = await api.audioDevices();
    } catch (e) {
      deviceMsg = String(e);
    }
  }

  async function pickOutput(e: Event) {
    const v = (e.currentTarget as HTMLSelectElement).value;
    deviceMsg = "切り替え中…";
    try {
      await api.setOutputDevice(v || null);
      settings.outputDevice = v;
      saveSettings();
      deviceMsg = null;
    } catch (err) {
      deviceMsg = `切り替えられませんでした(既定に戻しました): ${err}`;
      settings.outputDevice = "";
      saveSettings();
    }
    await loadDevices();
  }

  async function pickInput(e: Event) {
    const v = (e.currentTarget as HTMLSelectElement).value;
    try {
      await api.setInputDevice(v || null);
      settings.inputDevice = v;
      saveSettings();
      deviceMsg = null;
    } catch (err) {
      deviceMsg = String(err);
    }
    await loadDevices();
  }

  // ---- MIDI キーボード ----
  let midi = $state<{ inputs: string[]; current: string | null } | null>(null);
  let midiMsg = $state<string | null>(null);
  /** 直近に MIDI を受信したか(受信ランプ) */
  let midiActive = $state(false);

  async function loadMidi() {
    try {
      midi = await api.midiInputs();
    } catch (e) {
      midiMsg = String(e);
    }
  }

  async function pickMidi(e: Event) {
    const v = (e.currentTarget as HTMLSelectElement).value;
    try {
      await api.setMidiInput(v || null);
      settings.midiInput = v;
      saveSettings();
      midiMsg = null;
    } catch (err) {
      midiMsg = String(err);
    }
    await loadMidi();
  }

  // MIDI の受信表示(問い合わせは App が行い、ここは共有ストアを読む)
  $effect(() => {
    if (!midi?.current) return;
    return requestFastPolling();
  });
  $effect(() => {
    const idle = transportStore.state.midi_idle_ms;
    midiActive = !!midi?.current && idle != null && idle < 300;
  });

  // ---- 入力テスト(レベルメーター) ----
  let monitoring = $state(false);
  let levelDb = $state(-90);
  let levelPeakHold = $state(-90);

  async function toggleMonitor() {
    try {
      await api.inputMonitor(!monitoring);
      monitoring = !monitoring;
    } catch (e) {
      deviceMsg = String(e);
    }
  }

  $effect(() => {
    if (!monitoring) return;
    return requestFastPolling();
  });
  // 読み取りのたびに(seq が進むたびに)メーターを動かす
  $effect(() => {
    void transportStore.seq;
    if (!monitoring) return;
    const db = transportStore.state.input_peak_db ?? -90;
    // 表示は上がるときは即座に、下がるときはゆっくり
    levelDb = Math.max(db, untrack(() => levelDb) - 3);
    levelPeakHold = Math.max(db, untrack(() => levelPeakHold) - 0.5);
  });

  function meterPct(db: number): number {
    return Math.max(0, Math.min(100, ((db + 60) / 60) * 100));
  }

  const levelAdvice = $derived(
    levelPeakHold > -1
      ? "大きすぎます(音割れ)。マイクの音量を下げてください"
      : levelPeakHold > -12
        ? "ちょうど良い音量です"
        : levelPeakHold > -30
          ? "少し小さめです。もう少し上げると判定が安定します"
          : "かなり小さいです。Windows のマイク音量を上げてください",
  );

  // ---- 遅延の自動測定 ----
  let calib = $state<"idle" | "countin" | "tapping" | "analyzing">("idle");
  let calibMsg = $state<string | null>(null);
  let tapCount = $state(0);

  async function startCalibration() {
    if (monitoring) await toggleMonitor();
    calibMsg = null;
    try {
      const r = await api.calibrateStart();
      calib = "countin";
      const beatSecs = (r.total_secs - 0.4 - r.count_in_secs) / r.beats;
      setTimeout(() => {
        calib = "tapping";
        tapCount = 1;
        const iv = setInterval(() => {
          tapCount += 1;
          if (tapCount >= r.beats) clearInterval(iv);
        }, beatSecs * 1000);
      }, r.count_in_secs * 1000);
      setTimeout(finishCalibration, r.total_secs * 1000);
    } catch (e) {
      calib = "idle";
      calibMsg = String(e);
    }
  }

  async function finishCalibration() {
    calib = "analyzing";
    try {
      const r = await api.calibrateStop();
      settings.recordLatencyMs = Math.round(r.latency_ms / 5) * 5;
      saveSettings();
      calibMsg =
        `遅延 ${Math.round(r.latency_ms)}ms(${r.detected}/${r.beats} 拍を検出、ばらつき ±${Math.round(r.spread_ms)}ms)。` +
        (r.spread_ms > 40
          ? "ばらつきが大きいので、もう一度測ると精度が上がります。"
          : "レイテンシ補正に設定しました。");
    } catch (e) {
      calibMsg = String(e);
    }
    calib = "idle";
  }

  // ---- 追加モデル ----
  let clapModel = $state<api.ModelStatus | null>(null);
  let clapProgress = $state<{ got: number; total: number } | null>(null);
  let clapMsg = $state<string | null>(null);

  async function loadModels() {
    try {
      clapModel = (await api.modelStatus()).clap;
    } catch (e) {
      clapMsg = String(e);
    }
  }

  async function downloadClap() {
    if (clapProgress) return;
    clapMsg = null;
    clapProgress = { got: 0, total: clapModel?.bytes ?? 1 };
    const un = await api.onModelDownload((p) => (clapProgress = p));
    try {
      await api.downloadClapModel();
      clapMsg = "取得しました。AI が音を言葉でも捉えられるようになりました";
      await loadModels();
    } catch (e) {
      clapMsg = String(e);
    } finally {
      un();
      clapProgress = null;
    }
  }

  const mb = (n: number) => `${Math.round(n / 1_000_000)} MB`;

  // ---- AI(チャットの相手・モデル・MCP) ----
  const provider = $derived(settings.chatProvider);
  const currentModel = $derived(provider === "codex" ? settings.chatCodexModel : settings.chatModel);
  const isPresetModel = $derived(CHAT_MODELS[provider].some((m) => m.value === currentModel));
  /// 「その他」を選んでモデル名を入れているところ
  let customModel = $state(false);
  let modelName = $state("");

  function pickProvider(v: "claude" | "codex") {
    settings.chatProvider = v;
    customModel = false;
    saveSettings();
  }

  function pickModel(e: Event) {
    const v = (e.currentTarget as HTMLSelectElement).value;
    if (v === "__custom") {
      customModel = true;
      modelName = isPresetModel ? "" : currentModel;
      return;
    }
    customModel = false;
    setChatModel(v);
  }

  function commitModelName() {
    setChatModel(modelName.trim());
    customModel = false;
  }

  let info = $state<{ mcp_url: string } | null>(null);
  let copied = $state<string | null>(null);
  async function copy(what: string, text: string) {
    try {
      await navigator.clipboard.writeText(text);
      copied = what;
      setTimeout(() => (copied = null), 1500);
    } catch (e) {
      clapMsg = String(e);
    }
  }

  const TABS: { key: SettingsTab; label: string; icon: IconName }[] = [
    { key: "display", label: "表示", icon: "palette" },
    { key: "audio", label: "オーディオ", icon: "speaker" },
    { key: "midi", label: "MIDI", icon: "keyboard-music" },
    { key: "record", label: "録音", icon: "circle" },
    { key: "ai", label: "AI", icon: "sparkles" },
  ];

  onMount(() => {
    loadDevices();
    loadMidi();
    loadModels();
    api.appInfo().then((i) => (info = i)).catch(() => {});
    return () => {
      if (monitoring) api.inputMonitor(false).catch(() => {});
    };
  });

  function pickAccent(name: string) {
    settings.accent = name;
    applyTheme();
    saveSettings();
  }

  function toggleNotify(e: Event) {
    settings.notifyOnAiDone = (e.currentTarget as HTMLInputElement).checked;
    saveSettings();
    if (settings.notifyOnAiDone) playDoneChime(); // 試聴を兼ねる
  }
</script>

{#snippet row(title: string, desc: string | null)}
  <div class="sl">
    <div class="st">{title}</div>
    {#if desc}<div class="sd">{desc}</div>{/if}
  </div>
{/snippet}

<div class="backdrop" role="presentation" onclick={onClose}></div>
<div class="panel" role="dialog" aria-label="設定">
  <div class="head">
    <h2><Icon name="settings" />設定</h2>
    <button class="btn sm icon ghost" onclick={onClose} title="閉じる(Esc)" aria-label="閉じる"><Icon name="x" /></button>
  </div>

  <div class="body">
    <nav class="tabs" aria-label="設定の分類">
      {#each TABS as t (t.key)}
        <button class="tab" class:sel={settingsUi.tab === t.key} aria-current={settingsUi.tab === t.key} onclick={() => (settingsUi.tab = t.key)}>
          <Icon name={t.icon} />{t.label}
        </button>
      {/each}
    </nav>

    <div class="page">
      {#if settingsUi.tab === "display"}
        <h3>表示</h3>
        <div class="srow top">
          {@render row("テーマカラー", "ボタン・選択・再生ヘッドなどの色")}
          <div class="sc swatches">
            {#each ACCENT_PRESETS as p (p.name)}
              <button class="swatch" class:on={settings.accent === p.name} style="--sw:{p.accent}" onclick={() => pickAccent(p.name)} title={p.label}>
                <span class="dot"></span>{p.label}
              </button>
            {/each}
          </div>
        </div>
      {:else if settingsUi.tab === "audio"}
        <h3>
          オーディオ
          <button class="btn sm ghost" onclick={loadDevices} title="USB 機器を抜き差しした後など"><Icon name="refresh-cw" />一覧を更新</button>
        </h3>
        {#if devices}
          <div class="srow">
            {@render row(
              "出力(再生)",
              `使用中: ${devices.current_output ?? "なし"}${devices.sample_rate ? `(${(devices.sample_rate / 1000).toFixed(1)} kHz)` : ""}`,
            )}
            <select class="sc" value={settings.outputDevice} onchange={pickOutput} aria-label="出力デバイス">
              <option value="">OS の既定({devices.default_output ?? "なし"})</option>
              {#each devices.outputs as d (d)}<option value={d}>{d}</option>{/each}
            </select>
          </div>
          <div class="srow">
            {@render row("入力(録音)", `使用中: ${devices.current_input ?? "なし"}`)}
            <select class="sc" value={settings.inputDevice} onchange={pickInput} aria-label="入力デバイス">
              <option value="">OS の既定({devices.default_input ?? "なし"})</option>
              {#each devices.inputs as d (d)}<option value={d}>{d}</option>{/each}
            </select>
          </div>
        {:else}
          <div class="note">読み込み中…</div>
        {/if}
        <div class="srow">
          {@render row("入力テスト", "マイクの音量を確かめる(目安: 声の一番大きい所が -12〜-6dB の帯に入る)")}
          <div class="sc">
            <button class="btn sm" class:on={monitoring} onclick={toggleMonitor}
              ><Icon name={monitoring ? "square" : "mic"} />{monitoring ? "止める" : "始める"}</button
            >
          </div>
        </div>
        {#if monitoring}
          <div class="meter-row">
            <div class="meter" title={`直近のピーク ${levelPeakHold.toFixed(1)} dBFS`}>
              <div class="meter-fill" style="width:{meterPct(levelDb)}%"></div>
              <div class="meter-hold" style="left:{meterPct(levelPeakHold)}%"></div>
              <div class="meter-zone" style="left:{meterPct(-12)}%;width:{meterPct(-6) - meterPct(-12)}%"></div>
            </div>
            <div class="note">{levelPeakHold.toFixed(0)} dB — {levelAdvice}</div>
          </div>
        {/if}
        {#if deviceMsg}<div class="note warn">{deviceMsg}</div>{/if}
      {:else if settingsUi.tab === "midi"}
        <h3>
          MIDI キーボード
          <button class="btn sm ghost" onclick={loadMidi} title="USB 機器を抜き差しした後など"><Icon name="refresh-cw" />一覧を更新</button>
        </h3>
        {#if midi}
          <div class="srow">
            {@render row(
              "入力",
              midi.inputs.length === 0 ? "MIDI 機器が見つかりません。接続してから「一覧を更新」を押してください" : "鍵盤を弾くと右のランプが光る",
            )}
            <div class="sc">
              <span class="lamp" class:lit={midiActive} title="受信ランプ"></span>
              <select value={settings.midiInput} onchange={pickMidi} aria-label="MIDI 入力">
                <option value="">使わない</option>
                {#each midi.inputs as d (d)}<option value={d}>{d}</option>{/each}
                {#if settings.midiInput && !midi.inputs.includes(settings.midiInput)}
                  <option value={settings.midiInput}>{settings.midiInput}(未接続)</option>
                {/if}
              </select>
            </div>
          </div>
        {:else}
          <div class="note">読み込み中…</div>
        {/if}
        <div class="srow">
          {@render row("MIDI 録音の位置合わせ", "録ったノートの位置をそろえる")}
          <select
            class="sc"
            value={String(settings.midiQuantize)}
            onchange={(e) => {
              settings.midiQuantize = Number((e.currentTarget as HTMLSelectElement).value);
              saveSettings();
            }}
            aria-label="MIDI 録音の位置合わせ"
          >
            <option value="0">しない(弾いたまま)</option>
            <option value="240">16 分音符</option>
            <option value="480">8 分音符</option>
            <option value="160">3 連 8 分</option>
          </select>
        </div>
        <div class="note box">
          <Icon name="keyboard-music" size={14} />トラックの見出しの鍵盤のボタン(MIDI キーボードで弾く)で、鳴らすトラックを選びます
          (選んでいなければ、ピアノロールで開いているトラック → 最初の MIDI トラックの音)。そのボタンが ON のトラックがあるとき、録音は MIDI 録音になります。
        </div>
        {#if midiMsg}<div class="note warn">{midiMsg}</div>{/if}
      {:else if settingsUi.tab === "record"}
        <h3>録音</h3>
        <div class="srow">
          {@render row("カウントイン", "録音を始める前に鳴らす小節")}
          <select
            class="sc"
            value={String(settings.countInBars)}
            onchange={(e) => {
              settings.countInBars = Number((e.currentTarget as HTMLSelectElement).value);
              saveSettings();
            }}
            aria-label="カウントイン"
          >
            <option value="0">なし</option>
            <option value="1">1 小節</option>
            <option value="2">2 小節</option>
          </select>
        </div>
        <label class="srow">
          {@render row("録音中はメトロノームを鳴らす", null)}
          <input
            class="sc"
            type="checkbox"
            checked={settings.metronomeOnRecord}
            onchange={(e) => {
              settings.metronomeOnRecord = (e.currentTarget as HTMLInputElement).checked;
              saveSettings();
            }}
          />
        </label>
        <label class="srow">
          {@render row("録音の音量を自動で整える", "一番大きい所を -6dB に(元の録音は変えません)")}
          <input
            class="sc"
            type="checkbox"
            checked={settings.autoGain}
            onchange={(e) => {
              settings.autoGain = (e.currentTarget as HTMLInputElement).checked;
              saveSettings();
            }}
          />
        </label>
        <label class="srow">
          {@render row("ステレオで録音する", "入力が 2 ch 以上のとき。マイク 1 本ならオフのままで")}
          <input
            class="sc"
            type="checkbox"
            checked={settings.recordStereo}
            onchange={(e) => {
              settings.recordStereo = (e.currentTarget as HTMLInputElement).checked;
              saveSettings();
            }}
          />
        </label>
        <div class="srow">
          {@render row("レイテンシ補正", "録音が拍より遅れて置かれるなら増やし、早すぎるなら減らす")}
          <div class="sc">
            <input
              class="num"
              type="number"
              min="0"
              max="500"
              step="5"
              value={settings.recordLatencyMs}
              onchange={(e) => {
                settings.recordLatencyMs = Math.max(0, Number((e.currentTarget as HTMLInputElement).value) || 0);
                saveSettings();
              }}
              aria-label="レイテンシ補正(ms)"
            />
            ms
          </div>
        </div>
        <div class="srow">
          {@render row(
            "遅延を自動で測る",
            calib === "countin"
              ? "カウントイン中… 次の 1 小節から、クリックに合わせて手を叩くか「タッ」と言ってください"
              : calib === "tapping"
                ? `クリックに合わせて! ${tapCount} / 8`
                : calib === "analyzing"
                  ? "解析中…"
                  : (calibMsg ??
                    "メトロノームだけが鳴ります(曲は鳴りません)。スピーカーで聴いている場合は、クリック音がマイクに入るので叩かなくても測れます"),
          )}
          <div class="sc">
            <button class="btn sm" onclick={startCalibration} disabled={calib !== "idle"}><Icon name="target" />測る</button>
          </div>
        </div>
      {:else}
        <h3>AI</h3>
        <div class="srow">
          {@render row("チャットの相手", "この PC にインストールしてログインしておく。チャットの見出しでも切り替えられます")}
          <div class="sc seg">
            {#each CHAT_PROVIDERS as p (p.value)}
              <button class="btn sm" class:on={provider === p.value} onclick={() => pickProvider(p.value)} title={p.cli}>{p.label}<small>{p.cli}</small></button>
            {/each}
          </div>
        </div>
        <div class="srow">
          {@render row("モデル", "次の指示から使われます(会話の文脈はそのまま)")}
          <div class="sc col">
            <select value={customModel || !isPresetModel ? "__custom" : currentModel} onchange={pickModel} aria-label="モデル">
              {#each CHAT_MODELS[provider] as m (m.value)}<option value={m.value}>{m.label}</option>{/each}
              <option value="__custom">{!isPresetModel && !customModel ? `その他: ${currentModel}` : "その他(名前を入れる)"}</option>
            </select>
            {#if customModel}
              <!-- svelte-ignore a11y_autofocus -->
              <input
                type="text"
                placeholder={CHAT_MODEL_EXAMPLES[provider]}
                bind:value={modelName}
                autofocus
                onkeydown={(e) => {
                  if (e.key === "Enter" && !e.isComposing) commitModelName();
                  else if (e.key === "Escape") customModel = false;
                }}
                onblur={commitModelName}
              />
            {/if}
          </div>
        </div>
        <label class="srow">
          {@render row("作業が終わったら音で知らせる", "AI のターンが終わったとき(失敗したときは低い音)")}
          <input class="sc" type="checkbox" checked={settings.notifyOnAiDone} onchange={toggleNotify} />
        </label>

        <h3 class="sub">外から AI をつなぐ(MCP)</h3>
        <div class="srow">
          {@render row("MCP サーバー", "アプリの起動中、ほかの AI クライアントからこのアドレスでつなげます")}
          <code class="sc url">{info?.mcp_url ?? "…"}</code>
        </div>
        <div class="srow">
          {@render row("登録の仕方", "コピーして、ターミナル(Claude Code)か設定ファイル(Codex の ~/.codex/config.toml)に貼る")}
          <div class="sc">
            <button class="btn sm" disabled={!info} onclick={() => info && copy("claude", `claude mcp add --transport http glaux ${info.mcp_url}`)}
              ><Icon name={copied === "claude" ? "check" : "copy"} />Claude Code</button
            >
            <button class="btn sm" disabled={!info} onclick={() => info && copy("codex", `[mcp_servers.glaux]\nurl = "${info.mcp_url}"`)}
              ><Icon name={copied === "codex" ? "check" : "copy"} />Codex</button
            >
          </div>
        </div>

        <h3 class="sub">追加モデル</h3>
        <div class="srow">
          {@render row("音色を言葉で捉えるモデル(CLAP)", "AI が「明るい」「こもった」などの言葉で音色を比べられるようになる")}
          <div class="sc">
            {#if clapModel?.available}
              <span class="ok"><Icon name="check" size={14} />取得済み</span>
            {:else if clapProgress}
              <span class="note">取得中… {mb(clapProgress.got)} / {mb(clapProgress.total)}</span>
            {:else}
              <button class="btn sm" onclick={downloadClap} disabled={!clapModel}
                ><Icon name="download" />取得する({clapModel ? mb(clapModel.bytes) : "…"})</button
              >
            {/if}
          </div>
        </div>
        {#if clapProgress}<progress max={clapProgress.total} value={clapProgress.got}></progress>{/if}
        {#if clapMsg}<div class="note">{clapMsg}</div>{/if}
      {/if}
    </div>
  </div>
  <div class="foot">設定はこの PC に保存されます(プロジェクトには含まれません)</div>
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
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    width: 720px;
    max-width: calc(100vw - 32px);
    height: min(560px, calc(100vh - 64px));
    display: flex;
    flex-direction: column;
    background: var(--bg-panel);
    border: 1px solid var(--border-strong);
    border-radius: var(--r-lg);
    box-shadow: var(--shadow-pop);
    overflow: hidden;
  }

  .head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 10px 12px 10px 16px;
    border-bottom: 1px solid var(--border);
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

  .body {
    flex: 1;
    display: flex;
    min-height: 0;
  }

  .tabs {
    width: 160px;
    flex-shrink: 0;
    padding: 8px;
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .tab {
    display: flex;
    align-items: center;
    gap: 10px;
    border: 0;
    background: none;
    text-align: left;
    padding: 7px 10px;
    border-radius: var(--r-sm);
    font-size: var(--fs-md);
    color: var(--text-dim);
  }

  .tab:hover:not(:disabled) {
    border: 0;
    background: var(--bg-raised);
    color: var(--text);
  }

  .tab.sel {
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    color: var(--accent);
  }

  .page {
    flex: 1;
    overflow-y: auto;
    padding: 4px 20px 16px;
  }

  h3 {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin: 12px 0 4px;
    font-size: var(--fs-md);
    color: var(--text);
  }

  h3.sub {
    margin-top: 22px;
    padding-top: 12px;
    border-top: 1px solid var(--border);
    color: var(--text-dim);
    font-weight: 600;
  }

  /* 1 行 = 左に名前と説明、右に操作 */
  .srow {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 10px 0;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 60%, transparent);
  }

  .srow.top {
    align-items: flex-start;
  }

  label.srow {
    cursor: pointer;
  }

  .sl {
    flex: 1;
    min-width: 0;
  }

  .st {
    font-size: var(--fs-md);
  }

  .sd {
    margin-top: 2px;
    font-size: var(--fs-xs);
    color: var(--text-dim);
    line-height: 1.5;
  }

  .sc {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--fs-sm);
    color: var(--text-dim);
  }

  .sc.col {
    flex-direction: column;
    align-items: stretch;
  }

  select,
  input[type="text"],
  .num {
    height: 26px;
    background: var(--bg-inset);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    font-size: var(--fs-sm);
    padding: 0 6px;
  }

  select.sc,
  .sc select,
  .sc input[type="text"] {
    width: 270px;
  }

  .num {
    width: 72px;
  }

  input[type="checkbox"].sc {
    width: 16px;
    height: 16px;
    accent-color: var(--accent);
  }

  .seg .btn {
    height: auto;
    min-height: 26px;
    padding: 3px 10px;
    flex-direction: column;
    gap: 0;
  }

  .seg small {
    font-size: 9px;
    color: var(--text-faint);
  }

  .url {
    font-size: var(--fs-xs);
    color: var(--text);
    background: var(--bg-inset);
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    padding: 3px 6px;
  }

  .swatches {
    display: grid;
    grid-template-columns: repeat(3, 110px);
    gap: 6px;
  }

  .swatch {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 5px 8px;
    font-size: var(--fs-sm);
    background: var(--bg-raised);
    border-radius: var(--r-md);
  }

  .swatch.on {
    border-color: var(--sw);
    color: var(--text);
  }

  .swatch .dot {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: var(--sw);
    flex-shrink: 0;
  }

  .lamp {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--bg-raised);
    border: 1px solid var(--border);
  }

  .lamp.lit {
    background: var(--ok);
    border-color: var(--ok);
    box-shadow: 0 0 6px var(--ok);
  }

  .meter-row {
    padding: 4px 0 10px;
  }

  .meter {
    position: relative;
    height: 10px;
    border-radius: 3px;
    background: var(--bg-inset);
    border: 1px solid var(--border);
    overflow: hidden;
    margin-bottom: 4px;
  }

  .meter-fill {
    position: absolute;
    inset: 0 auto 0 0;
    background: var(--ok);
  }

  .meter-hold {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 2px;
    background: var(--warn);
  }

  .meter-zone {
    position: absolute;
    top: 0;
    bottom: 0;
    border-left: 1px dashed var(--text-dim);
    border-right: 1px dashed var(--text-dim);
  }

  .note {
    font-size: var(--fs-xs);
    color: var(--text-dim);
    line-height: 1.6;
  }

  .note.box {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    margin-top: 12px;
    padding: 8px 10px;
    border-radius: var(--r-md);
    background: var(--bg-raised);
  }

  .note.warn {
    color: var(--warn);
  }

  .ok {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--ok);
  }

  progress {
    width: 100%;
    accent-color: var(--accent);
  }

  .foot {
    padding: 8px 16px;
    border-top: 1px solid var(--border);
    font-size: var(--fs-xs);
    color: var(--text-faint);
  }
</style>
