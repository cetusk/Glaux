<script lang="ts">
  import {
    ACCENT_PRESETS,
    applyTheme,
    playDoneChime,
    saveSettings,
    settings,
  } from "./settings.svelte";

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

  onMount(() => {
    loadDevices();
    loadMidi();
    loadModels();
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

<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
<div class="backdrop" onclick={onClose}></div>
<div class="panel">
  <div class="head">
    <h2>⚙ 設定</h2>
    <button onclick={onClose} title="閉じる">✕</button>
  </div>

  <div class="section">
    <div class="section-title">テーマカラー</div>
    <div class="swatches">
      {#each ACCENT_PRESETS as p (p.name)}
        <button
          class="swatch"
          class:active={settings.accent === p.name}
          style="--sw:{p.accent}"
          onclick={() => pickAccent(p.name)}
          title={p.label}
        >
          <span class="dot"></span>
          <span class="sw-label">{p.label}</span>
        </button>
      {/each}
    </div>
  </div>

  <div class="section">
    <div class="section-title">通知</div>
    <label class="row">
      <input
        type="checkbox"
        checked={settings.notifyOnAiDone}
        onchange={toggleNotify}
      />
      AI の作業完了時にチャイムを鳴らす
    </label>
  </div>

  <div class="section">
    <div class="section-title">
      オーディオデバイス
      <button class="mini" onclick={loadDevices} title="一覧を更新(USB 機器を抜き差しした後など)">🔄</button>
    </div>
    {#if devices}
      <label class="row col">
        <span>🔈 出力(再生)</span>
        <select value={settings.outputDevice} onchange={pickOutput}>
          <option value="">OS の既定({devices.default_output ?? "なし"})</option>
          {#each devices.outputs as d (d)}
            <option value={d}>{d}</option>
          {/each}
        </select>
      </label>
      <label class="row col">
        <span>🎤 入力(録音)</span>
        <select value={settings.inputDevice} onchange={pickInput}>
          <option value="">OS の既定({devices.default_input ?? "なし"})</option>
          {#each devices.inputs as d (d)}
            <option value={d}>{d}</option>
          {/each}
        </select>
      </label>
      <div class="hint">
        使用中: 🔈 {devices.current_output ?? "なし"}{devices.sample_rate
          ? `(${(devices.sample_rate / 1000).toFixed(1)} kHz)`
          : ""} / 🎤 {devices.current_input ?? "なし"}
      </div>
    {:else}
      <div class="hint">読み込み中…</div>
    {/if}
    {#if deviceMsg}
      <div class="hint warn">{deviceMsg}</div>
    {/if}

    <div class="row">
      <button class="mini" class:on={monitoring} onclick={toggleMonitor}>
        {monitoring ? "■ 入力テストを止める" : "🎤 入力テスト"}
      </button>
    </div>
    {#if monitoring}
      <div class="meter" title={`直近のピーク ${levelPeakHold.toFixed(1)} dBFS`}>
        <div class="meter-fill" style="width:{meterPct(levelDb)}%"></div>
        <div class="meter-hold" style="left:{meterPct(levelPeakHold)}%"></div>
        <div class="meter-zone" style="left:{meterPct(-12)}%;width:{meterPct(-6) - meterPct(-12)}%"></div>
      </div>
      <div class="hint">{levelPeakHold.toFixed(0)} dB — {levelAdvice}(目安: 声の一番大きい所が -12〜-6dB の帯に入る)</div>
    {/if}
  </div>

  <div class="section">
    <div class="section-title">
      MIDI キーボード
      <button class="mini" onclick={loadMidi} title="一覧を更新(USB 機器を抜き差しした後など)">🔄</button>
    </div>
    {#if midi}
      <label class="row col">
        <span>🎹 入力 <span class="lamp" class:lit={midiActive} title="受信ランプ(鍵盤を弾くと光る)"></span></span>
        <select value={settings.midiInput} onchange={pickMidi}>
          <option value="">使わない</option>
          {#each midi.inputs as d (d)}
            <option value={d}>{d}</option>
          {/each}
          {#if settings.midiInput && !midi.inputs.includes(settings.midiInput)}
            <option value={settings.midiInput}>{settings.midiInput}(未接続)</option>
          {/if}
        </select>
      </label>
      {#if midi.inputs.length === 0}
        <div class="hint">MIDI 機器が見つかりません。接続してから 🔄 を押してください。</div>
      {/if}
      <div class="hint">
        トラック見出しの 🎹 で鳴らすトラックを選びます(未選択なら既定の音色)。
        🎹 のトラックがあるとき ⏺ は MIDI 録音になります。
      </div>
    {:else}
      <div class="hint">読み込み中…</div>
    {/if}
    <label class="row">
      MIDI 録音の位置合わせ
      <select
        value={String(settings.midiQuantize)}
        onchange={(e) => {
          settings.midiQuantize = Number((e.currentTarget as HTMLSelectElement).value);
          saveSettings();
        }}
      >
        <option value="0">しない(弾いたまま)</option>
        <option value="240">16 分音符</option>
        <option value="480">8 分音符</option>
        <option value="160">3 連 8 分</option>
      </select>
    </label>
    {#if midiMsg}
      <div class="hint warn">{midiMsg}</div>
    {/if}
  </div>

  <div class="section">
    <div class="section-title">録音</div>
    <label class="row">
      カウントイン
      <select
        value={String(settings.countInBars)}
        onchange={(e) => {
          settings.countInBars = Number((e.currentTarget as HTMLSelectElement).value);
          saveSettings();
        }}
      >
        <option value="0">なし</option>
        <option value="1">1 小節</option>
        <option value="2">2 小節</option>
      </select>
    </label>
    <label class="row">
      <input
        type="checkbox"
        checked={settings.metronomeOnRecord}
        onchange={(e) => {
          settings.metronomeOnRecord = (e.currentTarget as HTMLInputElement).checked;
          saveSettings();
        }}
      />
      録音中はメトロノームを鳴らす
    </label>
    <label class="row">
      レイテンシ補正
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
      />
      ms
    </label>
    <div class="hint">
      録音が拍より遅れて置かれるなら値を増やし、早すぎるなら減らします。下の自動測定で決められます。
    </div>
    <div class="row">
      <button class="mini" onclick={startCalibration} disabled={calib !== "idle"}>🎯 遅延を自動測定</button>
    </div>
    {#if calib === "countin"}
      <div class="hint strong">カウントイン中… 次の 1 小節から、クリックに合わせて手を叩くか「タッ」と言ってください</div>
    {:else if calib === "tapping"}
      <div class="hint strong">👏 クリックに合わせて! {tapCount} / 8</div>
    {:else if calib === "analyzing"}
      <div class="hint">解析中…</div>
    {:else if calibMsg}
      <div class="hint">{calibMsg}</div>
    {:else}
      <div class="hint">
        メトロノームだけが鳴ります(曲は鳴りません)。スピーカーで聴いている場合は、クリック音がマイクに入るので叩かなくても測れます。
      </div>
    {/if}
    <label class="row">
      <input
        type="checkbox"
        checked={settings.autoGain}
        onchange={(e) => {
          settings.autoGain = (e.currentTarget as HTMLInputElement).checked;
          saveSettings();
        }}
      />
      録音の音量を自動で整える(一番大きい所を -6dB に。元の録音は変えません)
    </label>
    <label class="row">
      <input
        type="checkbox"
        checked={settings.recordStereo}
        onchange={(e) => {
          settings.recordStereo = (e.currentTarget as HTMLInputElement).checked;
          saveSettings();
        }}
      />
      ステレオで録音する(入力が 2 ch 以上のとき。マイク 1 本ならオフのままで)
    </label>
  </div>

  <div class="section">
    <div class="section-title">追加モデル</div>
    <div class="row">
      音色を言葉で捉えるモデル(CLAP)
      {#if clapModel?.available}
        <span class="ok">✓ 取得済み</span>
      {:else if clapProgress}
        <span class="progress">取得中… {mb(clapProgress.got)} / {mb(clapProgress.total)}</span>
      {:else}
        <button class="mini" onclick={downloadClap} disabled={!clapModel}>
          ⬇ 取得する({clapModel ? mb(clapModel.bytes) : "…"})
        </button>
      {/if}
    </div>
    {#if clapProgress}
      <progress max={clapProgress.total} value={clapProgress.got}></progress>
    {/if}
    <div class="hint">
      AI が音を「明るい」「金属的なベル」のような言葉で捉え、音色の近さを比べられるようになります
      (LAION-CLAP、Apache-2.0。Hugging Face から取得し、この PC の設定フォルダに保存します)。
    </div>
    {#if clapMsg}
      <div class="hint">{clapMsg}</div>
    {/if}
  </div>

  <div class="note">設定はこの PC に保存されます(プロジェクトには含まれません)。</div>
</div>

<style>
  .num {
    width: 64px;
    margin: 0 4px 0 8px;
  }

  .hint {
    font-size: 11px;
    color: var(--text-dim);
    margin-top: 4px;
    line-height: 1.5;
  }

  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 29;
    background: rgba(0, 0, 0, 0.45);
  }

  .col {
    flex-direction: column;
    align-items: stretch;
    gap: 3px;
  }

  .col select {
    width: 100%;
  }

  .col > span {
    text-align: left;
    align-self: flex-start;
  }

  .mini {
    font-size: 11px;
    padding: 2px 8px;
  }

  .mini.on {
    border-color: var(--accent-dim);
    color: var(--accent);
  }

  .warn {
    color: #e8a07c;
  }

  .strong {
    color: var(--accent);
    font-weight: 600;
  }

  .lamp {
    display: inline-block;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    margin-left: 4px;
    background: var(--border);
    vertical-align: middle;
  }
  .lamp.lit {
    background: var(--accent);
    box-shadow: 0 0 6px var(--accent);
  }

  .meter {
    position: relative;
    height: 10px;
    border-radius: 3px;
    background: var(--bg);
    border: 1px solid var(--border);
    overflow: hidden;
  }

  .meter-fill {
    position: absolute;
    inset: 0 auto 0 0;
    background: linear-gradient(90deg, #3aa876 0%, #3aa876 70%, #e0c050 85%, #e05555 100%);
  }

  .meter-hold {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 2px;
    background: #fff;
  }

  .meter-zone {
    position: absolute;
    top: 0;
    bottom: 0;
    border-left: 1px dashed rgba(255, 255, 255, 0.5);
    border-right: 1px dashed rgba(255, 255, 255, 0.5);
  }

  .panel {
    position: fixed;
    z-index: 30;
    top: 60px;
    right: 16px;
    width: 360px;
    max-height: calc(100vh - 80px);
    overflow-y: auto;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.55);
    padding: 14px;
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
    margin: 0;
    font-size: 15px;
  }

  .section {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .section-title {
    font-size: 11px;
    color: var(--text-dim);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .swatches {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 6px;
  }

  .swatch {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 6px 8px;
  }

  .swatch.active {
    border-color: var(--sw);
    color: var(--text);
  }

  .dot {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--sw);
    flex-shrink: 0;
  }

  .sw-label {
    font-size: 11px;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    cursor: pointer;
  }

  .note {
    font-size: 11px;
    color: var(--text-dim);
  }
</style>
