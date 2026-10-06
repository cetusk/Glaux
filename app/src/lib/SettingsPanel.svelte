<script lang="ts">
  import { tr } from "./i18n.svelte";
  import Icon from "./Icon.svelte";
  import { APP_VERSION, appVersionDetail } from "./appVersion";
  import {
    accentLabel,
    ACCENT_PRESETS,
    applyTheme,
    CHAT_MODELS,
    CHAT_PROVIDERS,
    chatModelText,
    effortLabel,
    effortsFor,
    playDoneChime,
    saveSettings,
    setChatEffort,
    setChatModel,
    settings,
    settingsUi,
    type SettingsTab,
    welcomeUi,
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
    deviceMsg = tr("切り替え中…", "Switching…");
    try {
      await api.setOutputDevice(v || null);
      settings.outputDevice = v;
      saveSettings();
      deviceMsg = null;
    } catch (err) {
      deviceMsg = tr(`切り替えられませんでした(既定に戻しました): ${err}`, `Couldn't switch (reverted to the default): ${err}`);
      settings.outputDevice = "";
      saveSettings();
    }
    await loadDevices();
  }

  const BUFFER_SIZES = [64, 128, 256, 512, 1024, 2048];
  /** 選べる大きさ(デバイスの範囲に入るもの。範囲が分からなければ全部) */
  const bufferChoices = $derived.by(() => {
    const b = devices?.buffer;
    return BUFFER_SIZES.filter((n) => (b?.min == null || n >= b.min) && (b?.max == null || n <= b.max));
  });
  const msOf = (frames: number) => (devices?.sample_rate ? (frames / devices.sample_rate) * 1000 : 0);
  async function pickBuffer(e: Event) {
    const v = Number((e.currentTarget as HTMLSelectElement).value);
    deviceMsg = tr("開き直し中…", "Reopening…");
    try {
      const r = await api.setBufferSize(v);
      settings.bufferFrames = v;
      saveSettings();
      deviceMsg =
        r.applied === null
          ? tr("このデバイスは大きさを指定できないため、OS に任せています", "This device doesn't accept a buffer size, so the OS decides")
          : null;
    } catch (err) {
      deviceMsg = tr(`変えられませんでした: ${err}`, `Couldn't change it: ${err}`);
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
      ? tr("大きすぎます(音割れ)。マイクの音量を下げてください", "Too loud (clipping). Lower the mic volume")
      : levelPeakHold > -12
        ? tr("ちょうど良い音量です", "Good level")
        : levelPeakHold > -30
          ? tr("少し小さめです。もう少し上げると判定が安定します", "A bit quiet. Raise it a little for more reliable detection")
          : tr("かなり小さいです。Windows のマイク音量を上げてください", "Very quiet. Raise the mic volume in Windows"),
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
        tr(
          `遅延 ${Math.round(r.latency_ms)}ms(${r.detected}/${r.beats} 拍を検出、ばらつき ±${Math.round(r.spread_ms)}ms)。`,
          `Latency ${Math.round(r.latency_ms)} ms (detected ${r.detected}/${r.beats} beats, spread ±${Math.round(r.spread_ms)} ms). `,
        ) +
        (r.spread_ms > 40
          ? tr("ばらつきが大きいので、もう一度測ると精度が上がります。", "The spread is large; measuring again will improve accuracy.")
          : tr("レイテンシ補正に設定しました。", "Set as latency compensation."));
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
      clapMsg = tr("取得しました。AI が音を言葉でも捉えられるようになりました", "Downloaded. The AI can now describe sounds in words too");
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
  const currentEffort = $derived(provider === "codex" ? settings.chatCodexEffort : settings.chatEffort);
  const efforts = $derived(effortsFor(provider, currentModel));

  function pickProvider(v: "claude" | "codex") {
    settings.chatProvider = v;
    saveSettings();
  }

  function pickModel(e: Event) {
    const v = (e.currentTarget as HTMLSelectElement).value;
    if (v !== "__current") setChatModel(v);
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

  const TABS = $derived<{ key: SettingsTab; label: string; icon: IconName }[]>([
    { key: "display", label: tr("一般", "General"), icon: "sliders-horizontal" },
    { key: "audio", label: tr("オーディオ", "Audio"), icon: "speaker" },
    { key: "midi", label: "MIDI", icon: "keyboard-music" },
    { key: "record", label: tr("録音", "Recording"), icon: "circle" },
    { key: "ai", label: "AI", icon: "sparkles" },
    { key: "about", label: tr("Glaux について", "About Glaux"), icon: "info" },
  ]);

  const REPO_URL = "https://github.com/cetusk/Glaux";
  let urlCopied = $state(false);
  async function copyRepoUrl() {
    try {
      await navigator.clipboard.writeText(REPO_URL);
      urlCopied = true;
      setTimeout(() => (urlCopied = false), 1500);
    } catch {
      // クリップボードが使えなければ何もしない(URL は選んでコピーできる)
    }
  }

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
<div class="panel" role="dialog" aria-label={tr("設定", "Settings")}>
  <div class="head">
    <h2><Icon name="settings" />{tr("設定", "Settings")}</h2>
    <button class="btn sm icon ghost" onclick={onClose} title={tr("閉じる(Esc)", "Close (Esc)")} aria-label={tr("閉じる", "Close")}><Icon name="x" /></button>
  </div>

  <div class="body">
    <nav class="tabs" aria-label={tr("設定の分類", "Settings categories")}>
      {#each TABS as t (t.key)}
        <button class="tab" class:sel={settingsUi.tab === t.key} aria-current={settingsUi.tab === t.key} onclick={() => (settingsUi.tab = t.key)}>
          <Icon name={t.icon} />{t.label}
        </button>
      {/each}
    </nav>

    <div class="page">
      {#if settingsUi.tab === "display"}
        <h3>{tr("一般", "General")}</h3>
        <div class="srow">
          {@render row(tr("言語", "Language"), tr("画面の表示と、AI の返答(チャットの返事・質問・AI が付ける名前)の言語", "Language of the app and of the AI's replies (chat, questions, and names it creates)"))}
          <div class="sc seg">
            {#each [{ v: "ja", l: "日本語" }, { v: "en", l: "English" }] as o (o.v)}
              <button
                class="btn sm"
                class:on={settings.lang === o.v}
                onclick={() => {
                  settings.lang = o.v as "ja" | "en";
                  saveSettings();
                }}>{o.l}</button
              >
            {/each}
          </div>
        </div>
        <div class="srow top">
          {@render row(tr("テーマカラー", "Theme color"), tr("ボタン・選択・再生ヘッドなどの色", "Color of buttons, selections, the playhead, etc."))}
          <div class="sc swatches">
            {#each ACCENT_PRESETS as p (p.name)}
              <button class="swatch" class:on={settings.accent === p.name} style="--sw:{p.accent}" onclick={() => pickAccent(p.name)} title={accentLabel(p)}>
                <span class="dot"></span>{accentLabel(p)}
              </button>
            {/each}
          </div>
        </div>
        <div class="srow">
          {@render row(
            tr("はじめの確認", "Setup check"),
            tr("音の出力・AI のチャット・SoundFont がそろっているかの確認と、デモ曲", "Check that audio output, AI chat, and SoundFont are ready, plus a demo song"),
          )}
          <div class="sc">
            <button
              class="btn sm"
              onclick={() => {
                welcomeUi.open = true;
                onClose();
              }}>{tr("開く", "Open")}</button
            >
          </div>
        </div>
      {:else if settingsUi.tab === "audio"}
        <h3>
          {tr("オーディオ", "Audio")}
          <button class="btn sm ghost" onclick={loadDevices} title={tr("USB 機器を抜き差しした後など", "E.g. after plugging in or unplugging a USB device")}
            ><Icon name="refresh-cw" />{tr("一覧を更新", "Refresh")}</button
          >
        </h3>
        {#if devices}
          <div class="srow">
            {@render row(
              tr("出力(再生)", "Output (playback)"),
              tr(
                `使用中: ${devices.current_output ?? "なし"}${devices.sample_rate ? `(${(devices.sample_rate / 1000).toFixed(1)} kHz)` : ""}`,
                `In use: ${devices.current_output ?? "none"}${devices.sample_rate ? ` (${(devices.sample_rate / 1000).toFixed(1)} kHz)` : ""}`,
              ),
            )}
            <select class="sc" value={settings.outputDevice} onchange={pickOutput} aria-label={tr("出力デバイス", "Output device")}>
              <option value="">{tr(`OS の既定(${devices.default_output ?? "なし"})`, `OS default (${devices.default_output ?? "none"})`)}</option>
              {#each devices.outputs as d (d)}<option value={d}>{d}</option>{/each}
            </select>
          </div>
          <div class="srow">
            {@render row(
              tr("バッファの大きさ", "Buffer size"),
              tr(
                `小さいほど鍵盤やつまみから音までが速く、大きいほど途切れにくい。途切れるときは大きく` +
                  (devices.buffer?.block ? `(いま 1 回に ${devices.buffer.block} フレーム ≒ ${msOf(devices.buffer.block).toFixed(1)} ms で処理)` : ""),
                `Smaller responds faster to keys and knobs; larger is less likely to drop out. Increase it if audio drops out` +
                  (devices.buffer?.block ? ` (now processing ${devices.buffer.block} frames ≈ ${msOf(devices.buffer.block).toFixed(1)} ms at a time)` : ""),
              ),
            )}
            <select class="sc" value={settings.bufferFrames || 512} onchange={pickBuffer} aria-label={tr("バッファの大きさ", "Buffer size")}>
              {#each bufferChoices as n (n)}
                <option value={n}
                  >{tr(`${n} フレーム(${msOf(n).toFixed(1)} ms)`, `${n} frames (${msOf(n).toFixed(1)} ms)`)}{n === 512 ? tr(" — 既定", " — default") : ""}</option
                >
              {/each}
            </select>
          </div>
          <div class="srow">
            {@render row(tr("入力(録音)", "Input (recording)"), tr(`使用中: ${devices.current_input ?? "なし"}`, `In use: ${devices.current_input ?? "none"}`))}
            <select class="sc" value={settings.inputDevice} onchange={pickInput} aria-label={tr("入力デバイス", "Input device")}>
              <option value="">{tr(`OS の既定(${devices.default_input ?? "なし"})`, `OS default (${devices.default_input ?? "none"})`)}</option>
              {#each devices.inputs as d (d)}<option value={d}>{d}</option>{/each}
            </select>
          </div>
        {:else}
          <div class="note">{tr("読み込み中…", "Loading…")}</div>
        {/if}
        <div class="srow">
          {@render row(
            tr("入力テスト", "Input test"),
            tr("マイクの音量を確かめる(目安: 声の一番大きい所が -12〜-6dB の帯に入る)", "Check the mic level (aim for the loudest part of your voice to land in the -12 to -6 dB band)"),
          )}
          <div class="sc">
            <button class="btn sm" class:on={monitoring} onclick={toggleMonitor}
              ><Icon name={monitoring ? "square" : "mic"} />{monitoring ? tr("止める", "Stop") : tr("始める", "Start")}</button
            >
          </div>
        </div>
        {#if monitoring}
          <div class="meter-row">
            <div class="meter" title={tr(`直近のピーク ${levelPeakHold.toFixed(1)} dBFS`, `Recent peak ${levelPeakHold.toFixed(1)} dBFS`)}>
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
          {tr("MIDI キーボード", "MIDI keyboard")}
          <button class="btn sm ghost" onclick={loadMidi} title={tr("USB 機器を抜き差しした後など", "E.g. after plugging in or unplugging a USB device")}
            ><Icon name="refresh-cw" />{tr("一覧を更新", "Refresh")}</button
          >
        </h3>
        {#if midi}
          <div class="srow">
            {@render row(
              tr("入力", "Input"),
              midi.inputs.length === 0
                ? tr("MIDI 機器が見つかりません。接続してから「一覧を更新」を押してください", "No MIDI devices found. Connect one, then press \"Refresh\"")
                : tr("鍵盤を弾くと右のランプが光る", "The lamp on the right lights up when you play"),
            )}
            <div class="sc">
              <span class="lamp" class:lit={midiActive} title={tr("受信ランプ", "Activity lamp")}></span>
              <select value={settings.midiInput} onchange={pickMidi} aria-label={tr("MIDI 入力", "MIDI input")}>
                <option value="">{tr("使わない", "None")}</option>
                {#each midi.inputs as d (d)}<option value={d}>{d}</option>{/each}
                {#if settings.midiInput && !midi.inputs.includes(settings.midiInput)}
                  <option value={settings.midiInput}>{settings.midiInput}{tr("(未接続)", " (not connected)")}</option>
                {/if}
              </select>
            </div>
          </div>
        {:else}
          <div class="note">{tr("読み込み中…", "Loading…")}</div>
        {/if}
        <div class="srow">
          {@render row(tr("MIDI 録音の位置合わせ", "MIDI record quantize"), tr("録ったノートの位置をそろえる", "Snap recorded notes to the grid"))}
          <select
            class="sc"
            value={String(settings.midiQuantize)}
            onchange={(e) => {
              settings.midiQuantize = Number((e.currentTarget as HTMLSelectElement).value);
              saveSettings();
            }}
            aria-label={tr("MIDI 録音の位置合わせ", "MIDI record quantize")}
          >
            <option value="0">{tr("しない(弾いたまま)", "Off (as played)")}</option>
            <option value="240">{tr("16 分音符", "1/16 note")}</option>
            <option value="480">{tr("8 分音符", "1/8 note")}</option>
            <option value="160">{tr("3 連 8 分", "1/8 triplet")}</option>
          </select>
        </div>
        <div class="note box">
          <Icon name="keyboard-music" size={14} />{tr(
            "トラックの見出しの鍵盤のボタン(MIDI キーボードで弾く)で、鳴らすトラックを選びます(選んでいなければ、ピアノロールで開いているトラック → 最初の MIDI トラックの音)。そのボタンが ON のトラックがあるとき、録音は MIDI 録音になります。",
            "Use the keyboard button in a track header (play with MIDI keyboard) to choose which track sounds (if none is chosen: the track open in the piano roll, then the first MIDI track). While any track has that button on, recording becomes MIDI recording.",
          )}
        </div>
        {#if midiMsg}<div class="note warn">{midiMsg}</div>{/if}
      {:else if settingsUi.tab === "record"}
        <h3>{tr("録音", "Recording")}</h3>
        <div class="srow">
          {@render row(tr("カウントイン", "Count-in"), tr("録音を始める前に鳴らす小節", "Bars played before recording starts"))}
          <select
            class="sc"
            value={String(settings.countInBars)}
            onchange={(e) => {
              settings.countInBars = Number((e.currentTarget as HTMLSelectElement).value);
              saveSettings();
            }}
            aria-label={tr("カウントイン", "Count-in")}
          >
            <option value="0">{tr("なし", "Off")}</option>
            <option value="1">{tr("1 小節", "1 bar")}</option>
            <option value="2">{tr("2 小節", "2 bars")}</option>
          </select>
        </div>
        <label class="srow">
          {@render row(tr("録音中はメトロノームを鳴らす", "Play metronome while recording"), null)}
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
          {@render row(tr("録音の音量を自動で整える", "Auto-level recordings"), tr("一番大きい所を -6dB に(元の録音は変えません)", "Sets the peak to -6 dB (the original recording is kept)"))}
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
          {@render row(tr("ステレオで録音する", "Record in stereo"), tr("入力が 2 ch 以上のとき。マイク 1 本ならオフのままで", "When the input has 2 or more channels. Leave off for a single mic"))}
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
          {@render row(
            tr("レイテンシ補正", "Latency compensation"),
            tr("録音が拍より遅れて置かれるなら増やし、早すぎるなら減らす", "Increase if recordings land behind the beat; decrease if they land too early"),
          )}
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
              aria-label={tr("レイテンシ補正(ms)", "Latency compensation (ms)")}
            />
            ms
          </div>
        </div>
        <div class="srow">
          {@render row(
            tr("遅延を自動で測る", "Measure latency"),
            calib === "countin"
              ? tr(
                  "カウントイン中… 次の 1 小節から、クリックに合わせて手を叩くか「タッ」と言ってください",
                  "Counting in… From the next bar, clap or say \"ta\" along with the click",
                )
              : calib === "tapping"
                ? tr(`クリックに合わせて! ${tapCount} / 8`, `Along with the click! ${tapCount} / 8`)
                : calib === "analyzing"
                  ? tr("解析中…", "Analyzing…")
                  : (calibMsg ??
                    tr(
                      "メトロノームだけが鳴ります(曲は鳴りません)。スピーカーで聴いている場合は、クリック音がマイクに入るので叩かなくても測れます",
                      "Only the metronome plays (not the song). If you're listening on speakers, the click reaches the mic, so you can measure without clapping",
                    )),
          )}
          <div class="sc">
            <button class="btn sm" onclick={startCalibration} disabled={calib !== "idle"}><Icon name="target" />{tr("測る", "Measure")}</button>
          </div>
        </div>
      {:else if settingsUi.tab === "ai"}
        <h3>AI</h3>
        <div class="srow">
          {@render row(
            tr("チャットの相手", "Chat AI"),
            tr("この PC にインストールしてログインしておく。チャットの見出しでも切り替えられます", "Install and sign in on this PC. You can also switch it in the chat header"),
          )}
          <div class="sc seg">
            {#each CHAT_PROVIDERS as p (p.value)}
              <button class="btn sm" class:on={provider === p.value} onclick={() => pickProvider(p.value)} title={p.cli}>{p.label}<small>{p.cli}</small></button>
            {/each}
          </div>
        </div>
        <div class="srow">
          {@render row(tr("モデル", "Model"), tr("次の指示から使われます(会話の文脈はそのまま)", "Used from your next instruction (the conversation context is kept)"))}
          <div class="sc col">
            <select value={isPresetModel ? currentModel : "__current"} onchange={pickModel} aria-label={tr("モデル", "Model")}>
              {#each CHAT_MODELS[provider] as m (m.value)}<option value={m.value}
                  >{chatModelText(m.label)}{m.note ? tr(`(${chatModelText(m.note)})`, ` (${chatModelText(m.note)})`) : ""}</option
                >{/each}
              {#if !isPresetModel}<option value="__current">{tr("以前の指定", "Previously set")}: {currentModel}</option>{/if}
            </select>
          </div>
        </div>
        <div class="srow">
          {@render row(
            tr("考える深さ(effort)", "Thinking depth (effort)"),
            tr("深いほど丁寧だが、時間と使用量が増える。選べる段階はモデルによって違います", "Deeper is more thorough but takes more time and usage. Available levels depend on the model"),
          )}
          <div class="sc col">
            <select
              value={efforts.includes(currentEffort) ? currentEffort : ""}
              onchange={(e) => setChatEffort((e.currentTarget as HTMLSelectElement).value)}
              aria-label={tr("考える深さ", "Thinking depth")}
            >
              <option value="">{tr("既定(モデルの標準)", "Default (model standard)")}</option>
              {#each efforts as ef (ef)}<option value={ef}>{effortLabel(ef)}</option>{/each}
            </select>
          </div>
        </div>
        <div class="srow">
          {@render row(
            tr("作る前に AI が質問する", "AI asks before creating"),
            tr(
              "曲の決め手(ジャンル・雰囲気・長さ・編成など)が指示から読み取れないとき、作る前に選択肢で尋ねます。「途中で尋ねない」なら、いつも AI が決め手を選んで最後まで作ります(1 回だけなら指示の頭に /goal)",
              "When key choices for the song (genre, mood, length, instrumentation, etc.) can't be read from your instruction, the AI asks with options before creating. With \"Never ask\", the AI always makes those choices itself and finishes (for a single request, start it with /goal)",
            ),
          )}
          <div class="sc seg">
            {#each [{ v: "auto", l: tr("必要なとき", "When needed") }, { v: "never", l: tr("途中で尋ねない", "Never ask") }] as o (o.v)}
              <button
                class="btn sm"
                class:on={settings.chatAsk === o.v}
                onclick={() => {
                  settings.chatAsk = o.v as "auto" | "never";
                  saveSettings();
                }}>{o.l}</button
              >
            {/each}
          </div>
        </div>
        <label class="srow">
          {@render row(tr("作業が終わったら音で知らせる", "Chime when AI finishes"), tr("AI のターンが終わったとき(失敗したときは低い音)", "When the AI's turn ends (a low tone if it fails)"))}
          <input class="sc" type="checkbox" checked={settings.notifyOnAiDone} onchange={toggleNotify} />
        </label>

        <h3 class="sub">{tr("外から AI をつなぐ(MCP)", "Connect external AI (MCP)")}</h3>
        <div class="srow">
          {@render row(tr("MCP サーバー", "MCP server"), tr("アプリの起動中、ほかの AI クライアントからこのアドレスでつなげます", "While the app is running, other AI clients can connect at this address"))}
          <code class="sc url">{info?.mcp_url ?? "…"}</code>
        </div>
        <div class="srow">
          {@render row(
            tr("登録の仕方", "How to register"),
            tr("コピーして、ターミナル(Claude Code)か設定ファイル(Codex の ~/.codex/config.toml)に貼る", "Copy and paste into a terminal (Claude Code) or a config file (Codex's ~/.codex/config.toml)"),
          )}
          <div class="sc">
            <button class="btn sm" disabled={!info} onclick={() => info && copy("claude", `claude mcp add --transport http glaux ${info.mcp_url}`)}
              ><Icon name={copied === "claude" ? "check" : "copy"} />Claude Code</button
            >
            <button class="btn sm" disabled={!info} onclick={() => info && copy("codex", `[mcp_servers.glaux]\nurl = "${info.mcp_url}"`)}
              ><Icon name={copied === "codex" ? "check" : "copy"} />Codex</button
            >
          </div>
        </div>

        <h3 class="sub">{tr("追加モデル", "Additional models")}</h3>
        <div class="srow">
          {@render row(
            tr("音色を言葉で捉えるモデル(CLAP)", "Sound-to-words model (CLAP)"),
            tr("AI が「明るい」「こもった」などの言葉で音色を比べられるようになる", "Lets the AI compare sounds using words like \"bright\" or \"muffled\""),
          )}
          <div class="sc">
            {#if clapModel?.available}
              <span class="ok"><Icon name="check" size={14} />{tr("取得済み", "Downloaded")}</span>
            {:else if clapProgress}
              <span class="note">{tr("取得中…", "Downloading…")} {mb(clapProgress.got)} / {mb(clapProgress.total)}</span>
            {:else}
              <button class="btn sm" onclick={downloadClap} disabled={!clapModel}
                ><Icon name="download" />{tr(`取得する(${clapModel ? mb(clapModel.bytes) : "…"})`, `Download (${clapModel ? mb(clapModel.bytes) : "…"})`)}</button
              >
            {/if}
          </div>
        </div>
        {#if clapProgress}<progress max={clapProgress.total} value={clapProgress.got}></progress>{/if}
        {#if clapMsg}<div class="note">{clapMsg}</div>{/if}
      {:else if settingsUi.tab === "about"}
        <div class="about">
          <img class="about-logo" src="/glaux-logo.png" alt="Glaux" width="320" height="132" />
          <div class="about-ver" title={appVersionDetail()}>{tr("バージョン", "Version")} {APP_VERSION}</div>
          <p>
            {tr(
              "Glaux は、AI と一緒に曲を作れる、シンプルで軽いデスクトップの DAW です。画面で手を動かしても、チャットで AI に頼んでも、同じ曲を同じように編集できます。",
              "Glaux is a simple, lightweight desktop DAW for making music together with AI. Whether you edit by hand or ask the AI in chat, you edit the same song in the same way.",
            )}
          </p>
          <ul>
            <li>
              {tr(
                "編集はすべて履歴に残り、人と AI のどちらが何をしたかが分かります。どの編集も後から打ち消せます",
                "Every edit is kept in the history, showing whether you or the AI made it. Any edit can be undone later",
              )}
            </li>
            <li>
              {tr(
                "AI はアプリ内のチャットからも、外の AI(Claude Code・Codex など)から MCP でもつなげます",
                "Use AI from the in-app chat, or connect external AI (Claude Code, Codex, etc.) via MCP",
              )}
            </li>
            <li>{tr("内蔵の音源とエフェクトに加えて、SoundFont と CLAP プラグインを使えます", "Besides the built-in instruments and effects, you can use SoundFonts and CLAP plugins")}</li>
            <li>
              {tr(
                "作った曲は WAV・FLAC・MIDI に書き出せるほか、Godot のゲームの中でそのまま鳴らせます",
                "Export songs to WAV, FLAC, or MIDI, or play them directly inside Godot games",
              )}
            </li>
          </ul>
          <p class="note">
            {tr(
              "名前はギリシャ語で「フクロウ」(γλαύξ)。知恵の象徴のフクロウのように、曲作りにそっと寄り添う道具を目指しています。",
              "The name is Greek for \"owl\" (γλαύξ). Like the owl, a symbol of wisdom, it aims to be a tool that quietly supports your music-making.",
            )}
          </p>
          <div class="srow">
            {@render row(tr("ソースコード・使い方・更新", "Source code, docs, and updates"), tr("不具合の報告や要望もこちらへ", "Bug reports and requests are welcome here too"))}
            <div class="sc about-link">
              <span class="url">{REPO_URL}</span>
              <button class="btn sm icon ghost" onclick={copyRepoUrl} title={tr("URL をコピー", "Copy URL")} aria-label={tr("URL をコピー", "Copy URL")}
                ><Icon name={urlCopied ? "check" : "copy"} /></button
              >
            </div>
          </div>
          <div class="srow">
            {@render row(
              tr("ライセンス", "License"),
              tr("同梱している素材(アイコン・音源・学習済みモデル・グルーブの型)の出典は README に記載", "Sources of bundled assets (icons, sounds, trained models, groove templates) are listed in the README"),
            )}
            <div class="sc">{tr("MIT または Apache-2.0", "MIT or Apache-2.0")}</div>
          </div>
        </div>
      {/if}
    </div>
  </div>
  <div class="foot">{tr("設定はこの PC に保存されます(プロジェクトには含まれません)", "Settings are saved on this PC (not in the project)")}</div>
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
  .sc select {
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

  .about {
    padding-top: 12px;
    line-height: 1.7;
  }

  /* ロゴは白背景の版をそのまま(ブランドガイドの「白い背景ごと表示」) */
  .about-logo {
    display: block;
    width: 320px;
    max-width: 100%;
    height: auto;
    border-radius: var(--r-md);
  }

  .about-ver {
    margin: 8px 0 4px;
    font-size: var(--fs-sm);
    color: var(--text-dim);
  }

  .about p,
  .about ul {
    margin: 8px 0;
  }

  .about ul {
    padding-left: 20px;
  }

  .about-link {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .about-link .url {
    user-select: text;
    font-family: var(--font-mono, monospace);
    font-size: var(--fs-sm);
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
