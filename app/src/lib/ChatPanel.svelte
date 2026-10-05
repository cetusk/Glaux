<script lang="ts">
  import Icon from "./Icon.svelte";
  import { onMount, tick, untrack } from "svelte";
  import * as api from "./api";
  import { chatStatus } from "./aiStatus.svelte";
  import { toolShort } from "./toolLabels";
  import { clearAiHighlight, setAiHighlight } from "./aiHighlight.svelte";
  import { renderMarkdown } from "./markdown";
  import { showError, showToast } from "./toast.svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { claimAb, endAbLoop, ensureAbPlaying, releaseAb, restartAb, startAbLoop } from "./abLoop";
  import { defaultAbRange, rangeFromChanges } from "./abRange";
  import type { Project } from "./types";
  import {
    CHAT_MODELS,
    CHAT_PROVIDERS,
    EFFORT_LABELS,
    effortsFor,
    playDoneChime,
    playErrorChime,
    saveSettings,
    setChatEffort,
    setChatModel,
    settings,
  } from "./settings.svelte";

  const PROVIDERS = CHAT_PROVIDERS;
  const MODELS = CHAT_MODELS;
  const provider = $derived(settings.chatProvider);
  const currentModel = $derived(provider === "codex" ? settings.chatCodexModel : settings.chatModel);
  /// 一覧に無いモデル(以前に名前を手で入れたもの)。選び直すまでは残して見せる
  const isCustomModel = $derived(!MODELS[provider].some((m) => m.value === currentModel));
  const currentEffort = $derived(provider === "codex" ? settings.chatCodexEffort : settings.chatEffort);
  const efforts = $derived(effortsFor(provider, currentModel));

  function pickProvider(e: Event) {
    const v = (e.currentTarget as HTMLSelectElement).value;
    settings.chatProvider = v === "codex" ? "codex" : "claude";
    saveSettings();
  }

  function pickModel(e: Event) {
    const v = (e.currentTarget as HTMLSelectElement).value;
    if (v !== "__current") setChatModel(v);
  }

  function pickEffort(e: Event) {
    setChatEffort((e.currentTarget as HTMLSelectElement).value);
  }
  import { MASTER_FOCUS_ID, pianoRollStore, selectionStore, soundDesignStore, viewStore } from "./selection.svelte";
  import {
    MAX_PROPOSALS_AB,
    abLetter,
    abProposals,
    adoptProposal,
    designSel,
    designStore,
    designWatch,
    designTargetLabel,
    designTargetPrompt,
    endProposalAb,
    proposalAb,
    proposalsRange,
    refreshDesign,
    restartProposalAb,
    setProposalSide,
  } from "./design.svelte";

  /** 曲(ターンの聴き比べの範囲を拍子どおりに決めるため) */
  let { project = null }: { project?: Project | null } = $props();

  // 設計画面で選んでいる所(設計画面を開いている間だけ、指示の対象として添える)。
  // 対象の ✕ は、設計画面の選択を残したままチャットに添えるのだけをやめる(別の所を選ぶと、また添える)
  let designDetached = $state<object | null>(null);
  const designLabel = $derived(viewStore.main === "design" ? designTargetLabel(designStore.data, designSel.sel) : null);
  const designTarget = $derived(designLabel && designSel.sel !== designDetached ? designLabel : null);

  interface Msg {
    role: "user" | "assistant" | "tool" | "notice" | "error" | "turn" | "choices" | "question";
    text: string;
    /** role "turn": ターンの開始前の最後の履歴エントリ(取り消しの起点)と、取り消したか */
    since?: string | null;
    /** role "turn": ターンで AI が最初にした編集(聴き比べの「前」= この編集の前) */
    first?: string;
    /** role "choices": このターンで AI が出した案(出した順) */
    planIds?: string[];
    reverted?: boolean;
    /** role "user": 送信待ちのまま停止したので送らなかった */
    dropped?: boolean;
    /** role "question": AI の質問(ask_user)と、送った答え・答えずに次へ進んだか */
    question?: api.AiQuestion;
    answer?: string;
    skipped?: boolean;
  }

  /// 送信待ちの指示(実行中に書いたもの)。会話の下に並べ、前のターンが終わったら順に送る
  interface Pending {
    text: string;
    fullPrompt: string;
  }
  let pending = $state<Pending[]>([]);

  /// 実行中のターンの開始前の最後の履歴エントリ(null = 履歴が空)
  let turnStart: string | null = null;

  /// 実行中のターンを始めた時刻と、そのターンで AI が案を出したか
  let turnStartedAt = 0;
  let turnProposed = false;

  /// ターンが終わったら、AI の編集の件数を数えて「取り消す」「聴き比べる」を出し、変わった所を縁取る。
  /// 案を出していたら、その案の選択肢(聴き比べる・採用)を並べる
  async function finishTurn() {
    try {
      const ch = await api.turnChanges(turnStart);
      if (ch.entry_ids.length > 0) {
        setAiHighlight(ch);
        push({ role: "turn", text: `このターンの編集 ${ch.entry_ids.length} 件`, since: turnStart, first: ch.entry_ids[0] });
      }
    } catch {
      // 数えられなくても会話には影響しない
    }
    if (turnProposed) {
      turnProposed = false;
      try {
        await refreshDesign();
        const d = designStore.data;
        const live = new Set((d?.plans ?? []).filter((p) => p.state === "proposal").map((p) => p.plan_id));
        // 計画の履歴は新しい順。このターンで作られた案を、出した順に
        const ids = (d?.history ?? [])
          .filter((h) => h.op === "create" && live.has(h.plan_id) && Date.parse(h.time) >= turnStartedAt - 2000)
          .map((h) => h.plan_id)
          .reverse();
        const uniq = [...new Set(ids)];
        if (uniq.length) push({ role: "choices", text: "", planIds: uniq });
      } catch {
        // 案を並べられなくても、設計画面から聴き比べられる
      }
    }
  }

  // ---- このターンの前と今を、音量をそろえて聴き比べる(A = ターンの前、B = 今) ----
  let turnAb = $state<{ key: string; side: "a" | "b"; info: api.AbInfo; start: number; end: number } | null>(null);
  let turnAbBusy = $state<string | null>(null);
  const turnKey = (m: Msg) => m.first ?? m.since ?? "start";

  async function startTurnAb(m: Msg) {
    if (turnAbBusy) return;
    const key = turnKey(m);
    turnAbBusy = key;
    claimAb("chat", endTurnAb);
    try {
      let first = m.first;
      if (!first) first = (await api.turnChanges(m.since ?? null)).entry_ids[0];
      if (!first) throw new Error("このターンの編集が見つかりません");
      // 範囲: このターンで音が変わった所(最初の所から)。分からなければ今の位置から
      let range: { start: number; end: number } | null = null;
      let r: { ranges: [number, number][]; whole: boolean; silent_only?: boolean } | null = null;
      try {
        r = await invoke<{ ranges: [number, number][]; whole: boolean; silent_only?: boolean }>("change_ranges", { beforeEntry: first });
        if (project) range = rangeFromChanges(project, r.ranges);
      } catch {
        range = null;
      }
      if (r?.silent_only) {
        // 鳴っていないトラックだけの編集: 聴き比べても同じ音なので始めない
        releaseAb("chat");
        showToast("warn", "このターンの編集は、今鳴っていないトラック(ミュート中・ほかのトラックのソロ中)だけでした。聴き比べても違いはありません");
        return;
      }
      range ??= defaultAbRange(project);
      const info = await api.abPrepare(first, range.start, range.end);
      const end = info.end_tick && info.end_tick > range.start ? info.end_tick : range.end;
      turnAb = { key, side: "b", info, start: range.start, end };
      await startAbLoop(range.start, end);
      revealAb();
    } catch (e) {
      releaseAb("chat");
      showToast("error", `聴き比べを用意できませんでした: ${e}`);
    } finally {
      turnAbBusy = null;
    }
  }

  function setTurnSide(side: "a" | "b") {
    if (!turnAb) return;
    turnAb.side = side;
    api.abSetSide(side).catch(() => {});
    ensureAbPlaying(turnAb.start, turnAb.end).catch(() => {});
  }

  function endTurnAb() {
    releaseAb("chat");
    if (!turnAb) return;
    turnAb = null;
    api.abClear().catch(() => {});
    endAbLoop();
  }

  // 聴き比べの途中で曲が変わったら、「今」が古くなるので終える
  let abProject: Project | null = null;
  $effect(() => {
    const p = project;
    if (!turnAb) {
      abProject = p;
      return;
    }
    if (p !== abProject) {
      endTurnAb();
      showToast("warn", "曲が変わったので聴き比べを終えました(もう一度「聴き比べる」で聴けます)");
    }
  });

  // ---- このターンで AI が出した案の選択肢 ----
  // 一覧があれば、設計画面の外でも設計データ(案が残っているか)を読み直してもらう
  $effect(() => {
    const n = messages.filter((m) => m.role === "choices").length;
    designWatch.chat = n;
    if (n > 0 && !untrack(() => designStore.data)) refreshDesign();
  });
  const liveProposals = (ids: string[] | undefined) =>
    (ids ?? []).filter((id) => designStore.data?.plans.some((p) => p.plan_id === id && p.state === "proposal"));
  const planName = (id: string) => designStore.data?.plans.find((p) => p.plan_id === id)?.name;
  const comparing = (ids: string[]) => ids.length > 0 && proposalAb.planIds.length > 0 && proposalAb.planIds.every((id) => ids.includes(id));

  async function listenProposals(ids: string[]) {
    if (!designStore.data) await refreshDesign();
    const use = ids.slice(0, MAX_PROPOSALS_AB);
    const r = project ? proposalsRange(project, designStore.data, use) : { ...defaultAbRange(null), section: null };
    await abProposals(use, r.start, r.end, r.section);
    revealAb();
  }

  /** 聴き比べの操作(A / B …)が出たら、その行が見える所までスクロールする */
  async function revealAb() {
    await tick();
    scroller?.querySelector<HTMLElement>("[data-ab-live]")?.scrollIntoView({ block: "nearest", behavior: "smooth" });
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
  /** ターンの途中に出た答えていない質問を、ターンの終わりに会話の最後へ移す(入力欄の近くで答えられるように) */
  function questionsToEnd() {
    const open = messages.filter((m) => m.role === "question" && qOpen(m));
    if (!open.length) return;
    messages = [...messages.filter((m) => !(m.role === "question" && qOpen(m))), ...open];
    saveLog();
    revealQuestion();
  }

  /** 答えていない質問のカードを見せる(欄より高ければカードの頭から、収まれば最後まで) */
  async function revealQuestion() {
    await tick();
    const cards = scroller?.querySelectorAll<HTMLElement>(".msg.question:not(.closed)");
    const el = cards?.[cards.length - 1];
    if (el && scroller && el.offsetHeight > scroller.clientHeight) el.scrollIntoView({ block: "start" });
    else scrollToBottom(true);
  }
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
      for (const m of messages) if (m.role === "question" && m.question) initQuestion(m.question);
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

  // 入力欄は内容に合わせて高くなる(1 行〜5 行。それ以上は欄の中でスクロール)。
  // 上のハンドルで高さを決めたら、その高さに固定する(チャット欄の高さの中で。ダブルクリックで自動に戻す)
  const MAX_LINES = 5;
  /** 入力欄より上に最低限残す高さ(見出しと会話の表示) */
  const KEEP_ABOVE = 150;
  const INPUT_MIN = 40;
  let inputEl: HTMLTextAreaElement | undefined = $state();
  let chatH = $state(0);
  let inputH = $state<number | null>(loadInputH());
  function loadInputH(): number | null {
    try {
      const v = Number(localStorage.getItem("glaux.chatInputH"));
      return Number.isFinite(v) && v >= INPUT_MIN ? v : null;
    } catch {
      return null;
    }
  }
  /** チャット欄の高さに収めた入力欄の高さ(欄を縮めたときも入力欄だけが残らないように) */
  const inputShown = $derived(
    inputH == null ? null : Math.max(INPUT_MIN, Math.min(inputH, (chatH || 600) - KEEP_ABOVE)),
  );
  function startInputResize(e: PointerEvent) {
    e.preventDefault();
    const y0 = e.clientY;
    const h0 = inputShown ?? inputEl?.offsetHeight ?? 60;
    // ドラッグの間は文字を選ばない(会話の文字が反転しないように)
    const prevSelect = document.body.style.userSelect;
    document.body.style.userSelect = "none";
    window.getSelection()?.removeAllRanges();
    const move = (m: PointerEvent) => {
      inputH = Math.round(Math.max(INPUT_MIN, Math.min((chatH || 600) - KEEP_ABOVE, h0 - (m.clientY - y0))));
    };
    const up = () => {
      document.body.style.userSelect = prevSelect;
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      try {
        if (inputH != null) localStorage.setItem("glaux.chatInputH", String(inputH));
      } catch {
        // 保存できなくても動作には関係しない
      }
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }
  function resetInputH() {
    inputH = null;
    try {
      localStorage.removeItem("glaux.chatInputH");
    } catch {
      // 保存できなくても動作には関係しない
    }
  }
  /** キーボードでも動かせるように(上下の矢印で 20px ずつ) */
  function onResizeKey(e: KeyboardEvent) {
    if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
    e.preventDefault();
    const h0 = inputShown ?? inputEl?.offsetHeight ?? 60;
    inputH = Math.max(INPUT_MIN, Math.min((chatH || 600) - KEEP_ABOVE, h0 + (e.key === "ArrowUp" ? 20 : -20)));
    try {
      localStorage.setItem("glaux.chatInputH", String(inputH));
    } catch {
      // 保存できなくても動作には関係しない
    }
  }
  $effect(() => {
    void input;
    // 隠れていた欄が見えるようになったら(下のパネルを開いたときなど)測り直す。隠れている間は中身の高さが 0 と測られる
    void chatH;
    const el = inputEl;
    if (!el) return;
    if (inputShown != null) {
      el.style.height = `${inputShown}px`;
      el.style.overflowY = "auto";
      return;
    }
    const cs = getComputedStyle(el);
    const line = parseFloat(cs.lineHeight) || 18;
    const extra =
      parseFloat(cs.paddingTop) +
      parseFloat(cs.paddingBottom) +
      parseFloat(cs.borderTopWidth) +
      parseFloat(cs.borderBottomWidth);
    el.style.height = "auto";
    const max = line * MAX_LINES + extra;
    // 少なくとも 1 行分(隠れていて中身の高さが測れないときも、つぶれないように)
    const min = line + extra;
    el.style.height = `${Math.max(min, Math.min(el.scrollHeight + parseFloat(cs.borderTopWidth) + parseFloat(cs.borderBottomWidth), max))}px`;
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
    if (designTarget) {
      prefix += designTargetPrompt(designStore.data, designSel.sel) ?? "";
      shown = `〔${designTarget}〕 ${shown}`;
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
    // /goal: 質問せずに AI が決め手を選んで最後まで作る(何も無ければ、何でもよいので 1 曲)。設定で「尋ねない」なら毎回
    const goal = prompt.match(/^\/(goal|おまかせ)(?:\s+|$)([\s\S]*)$/);
    let body = prompt;
    if (goal) {
      body = goal[2].trim() || GOAL_DEFAULT;
      shown = shown.replace(prompt, `〔おまかせ〕 ${body}`);
    }
    if (goal || settings.chatAsk === "never") prefix = OMAKASE + prefix;
    const fullPrompt = prefix ? `${prefix}\n${body}` : body;
    rememberPrompt(prompt);
    input = "";
    await sendPrompt(shown, fullPrompt);
  }

  /** 送る(実行中なら送信待ちに並べる)。答えていない質問は「答えずに次へ進んだ」にする */
  async function sendPrompt(shown: string, fullPrompt: string) {
    for (const m of messages) if (m.role === "question" && !m.answer && !m.skipped) m.skipped = true;
    if (chatStatus.running || pending.length > 0) {
      // 実行中に書いた指示は、今のターンが終わってから送る
      pending.push({ text: shown, fullPrompt });
      scrollToBottom(true);
      return;
    }
    push({ role: "user", text: shown }, true);
    await start(fullPrompt);
  }

  // ---- AI からの質問(ask_user)----
  const OMAKASE =
    "【おまかせ】ask_user で質問せず、決め手(ジャンル・雰囲気・長さ・編成など)は自分で選んで最後まで作る。選んだ決め手は報告に書く。\n";
  const GOAL_DEFAULT = "何でもよいので、1 曲作ってください(ジャンル・雰囲気・長さ・編成は自由に選んでください)。";
  /** 質問ごとの選んだ選択肢とその他の言葉(質問の ID → 質問の番号ごと) */
  let qPick = $state<Record<string, { picks: string[]; other: string }[]>>({});
  /** 選んだ内容の入れ物を作る(質問が届いたとき・会話の記録を読んだとき。描画の途中では作らない) */
  function initQuestion(q: api.AiQuestion) {
    if (!qPick[q.id]) qPick[q.id] = q.questions.map(() => ({ picks: [], other: "" }));
  }
  const qState = (m: Msg) => qPick[m.question!.id] ?? m.question!.questions.map(() => ({ picks: [], other: "" }));
  const qOpen = (m: Msg) => !m.answer && !m.skipped;
  function togglePick(m: Msg, qi: number, label: string) {
    initQuestion(m.question!);
    const st = qPick[m.question!.id][qi];
    const multi = m.question!.questions[qi].multi;
    st.picks = st.picks.includes(label) ? st.picks.filter((x) => x !== label) : multi ? [...st.picks, label] : [label];
  }
  /** 答えを送る(`all` なら全部おまかせ)。選ばなかった質問は AI に任せる */
  async function answerQuestion(m: Msg, all = false) {
    if (!qOpen(m)) return;
    const q = m.question!;
    const st = qState(m);
    const lines = q.questions.map((qq, i) => {
      const v = all ? [] : [...st[i].picks, ...(st[i].other.trim() ? [st[i].other.trim()] : [])];
      return `- ${qq.header}: ${v.length ? v.join("・") : "おまかせ"}`;
    });
    const summary = all ? "すべておまかせ" : lines.map((l) => l.slice(2)).join(" / ");
    const shown = `【質問への答え】${summary}`;
    const full =
      (all ? OMAKASE : "") +
      `【質問への答え】\n${lines.join("\n")}\nこの答えをもとに、これ以上は質問せずに作る(「おまかせ」の項目は自分で選び、選んだものを報告に書く)。`;
    // 答えた質問は閉じる(sendPrompt がほかの答えていない質問を「進んだ」にする前に)
    m.answer = summary;
    saveLog();
    await sendPrompt(shown, full);
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
    turnStartedAt = Date.now();
    turnProposed = false;
    try {
      const h = await api.getHistory(1);
      turnStart = h.entries.length > 0 ? h.entries[h.entries.length - 1].id : null;
    } catch {
      turnStart = null;
    }
    try {
      await api.sendChat(fullPrompt, currentModel, provider, currentEffort);
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
    // 設計画面の「この所について AI に頼む」: 入力欄へ移る
    const focusChat = () => inputEl?.focus();
    window.addEventListener("glaux:focus-chat", focusChat);
    // 設計画面の「この版から作り直す」など: 決まった言葉をそのまま送る(選んでいる所は対象として添わる)
    const sendText = (e: Event) => {
      const text = (e as CustomEvent<string>).detail;
      if (!text) return;
      // 入力欄の書きかけは消さない: 決まった言葉だけを送り、書きかけは入力欄に戻す
      // (send は最初の await より前に入力欄を空にするので、呼んだ直後に戻せる)
      const draft = input;
      input = text;
      void send();
      if (draft.trim()) input = draft;
    };
    window.addEventListener("glaux:chat-send", sendText);
    // AI の質問(このチャットのターンの途中に来たものだけ。外の AI クライアントの質問は、その AI が文章でも示す)
    const unlistenQ = api
      .onAiQuestion((q) => {
        if (!chatStatus.running) return;
        initQuestion(q);
        push({ role: "question", text: "", question: q });
        revealQuestion();
      })
      .catch(() => undefined);
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
            if (ev.name.includes("propose_design")) turnProposed = true;
            break;
          case "result":
            chatStatus.running = false;
            // 成功時の最終テキストは assistant_text で受信済みなので出さない
            if (!ev.ok && ev.text) {
              push({ role: "error", text: ev.text });
            }
            if (ev.ok) playDoneChime();
            else playErrorChime();
            finishTurn().then(() => {
              questionsToEnd();
              sendNext();
            });
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
      window.removeEventListener("glaux:focus-chat", focusChat);
      window.removeEventListener("glaux:chat-send", sendText);
      unlisten.then((f) => f && f());
      unlistenQ.then((f) => f && f());
    };
  });
</script>

<div class="chat" bind:clientHeight={chatH}>
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
          <option value={m.value} title={m.note}>{m.label}</option>
        {/each}
        {#if isCustomModel}
          <option value="__current">{currentModel}</option>
        {/if}
      </select>
      <select
        class="effort"
        value={efforts.includes(currentEffort) ? currentEffort : ""}
        onchange={pickEffort}
        disabled={chatStatus.running}
        title="考える深さ(effort)。深いほど丁寧だが時間と使用量が増える。次の指示から反映"
        aria-label="考える深さ"
      >
        <option value="">深さ: 既定</option>
        {#each efforts as ef (ef)}
          <option value={ef}>{EFFORT_LABELS[ef] ?? ef}</option>
        {/each}
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
              ><Icon name="eraser" />このターンを取り消す</button
            >
            {#if turnAb?.key !== turnKey(m)}
              <button
                class="btn sm"
                disabled={turnAbBusy != null}
                onclick={() => startTurnAb(m)}
                title="このターンの前と今を、音量をそろえて切り替えて聴く(範囲はこのターンで音が変わった所)"
                ><Icon name="headphones" />{turnAbBusy === turnKey(m) ? "用意しています…" : "前と今を聴き比べる"}</button
              >
            {/if}
          {/if}
        </div>
        {#if turnAb && turnAb.key === turnKey(m)}
          <div class="msg abrow" data-ab-live role="group" aria-label="このターンの前と今を切り替える">
            <button class="btn sm" class:on={turnAb.side === "a"} type="button" onclick={() => setTurnSide("a")}>A 前</button>
            <button class="btn sm" class:on={turnAb.side === "b"} type="button" onclick={() => setTurnSide("b")}>B 今</button>
            <button class="btn sm" type="button" title="範囲の頭から聴き直す" onclick={() => turnAb && restartAb(turnAb.start)}>⏮ 頭から</button>
            <button class="btn sm" type="button" onclick={endTurnAb}>終える</button>
            {#if turnAb.info.first_diff_secs == null}
              <span class="abnote warn"
                >この範囲では、前と今の音が同じです。このターンの編集は、ミュートしたトラックなど今は聞こえない所だけかもしれません</span
              >
            {:else}
              <span class="abnote">音量はそろえてあります。切り替えても同じ位置から続けて鳴ります</span>
            {/if}
          </div>
        {/if}
      {:else if m.role === "choices"}
        {@const live = liveProposals(m.planIds)}
        {@const on = comparing(live)}
        <div class="msg choices" data-ab-live={on ? "" : undefined}>
          <div class="ch-head"><Icon name="split" size={13} />AI の案({m.planIds?.length ?? 0})</div>
          {#each m.planIds ?? [] as id (id)}
            {@const k = live.indexOf(id)}
            <div class="ch-row">
              {#if k >= 0}
                <span class="pletter" class:cur={on && proposalAb.side === proposalAb.planIds.indexOf(id) + 1}>{abLetter(k + 1)}</span>
                <span class="ch-name">{planName(id)}</span>
                <span class="spacer"></span>
                <button class="btn sm primary" type="button" title="案の音を曲に当て、案の計画を今の計画にする(同じ頼みのほかの案は片付ける)" onclick={() => adoptProposal(id, planName(id) ?? "案")}
                  >採用</button
                >
              {:else}
                <span class="ch-gone">採用したか、片付けた案</span>
              {/if}
            </div>
          {/each}
          {#if live.length}
            <div class="ch-row">
              {#if on}
                <span class="abswitch" role="group" aria-label="今と案を切り替える">
                  <button class="btn sm" class:on={proposalAb.side === 0} type="button" onclick={() => setProposalSide(0)}>A 今</button>
                  {#each proposalAb.planIds as pid, j (pid)}
                    <button class="btn sm" class:on={proposalAb.side === j + 1} type="button" title={planName(pid)} onclick={() => setProposalSide(j + 1)}
                      >{abLetter(j + 1)}</button
                    >
                  {/each}
                </span>
                <button class="btn sm" type="button" title="範囲の頭から聴き直す" onclick={restartProposalAb}>⏮ 頭から</button>
                <button class="btn sm" type="button" onclick={() => endProposalAb()}>終える</button>
              {:else}
                <button class="btn sm" type="button" disabled={proposalAb.busy != null} onclick={() => listenProposals(live)}
                  ><Icon name="headphones" />{live.length >= 2 ? `まとめて聴き比べる(今 + 案 ${Math.min(live.length, MAX_PROPOSALS_AB)} つ)` : "今と聴き比べる"}</button
                >
              {/if}
              <button class="btn sm" type="button" onclick={() => (viewStore.main = "design")}>設計画面で見る</button>
            </div>
            {#if on}<div class="abnote">音量はいちばん小さいものにそろえてあります。切り替えても同じ位置から続けて鳴ります</div>{/if}
          {:else}
            <div class="abnote">どの案も、もう残っていません</div>
          {/if}
        </div>
      {:else if m.role === "question" && m.question}
        {@const open = qOpen(m)}
        {@const st = qState(m)}
        <div class="msg question" class:closed={!open} role="group" aria-label="AI からの質問">
          <div class="ch-head"><Icon name="message-circle-question-mark" size={13} />AI からの質問{m.question.why ? `(${m.question.why})` : ""}</div>
          {#each m.question.questions as q, qi (qi)}
            <div class="q-item">
              <div class="q-text"><span class="q-tag">{q.header}</span>{q.question}{q.multi ? "(いくつでも)" : ""}</div>
              <div class="q-opts">
                {#each q.options as o (o.label)}
                  <button
                    class="btn sm q-opt"
                    class:on={st[qi].picks.includes(o.label)}
                    type="button"
                    disabled={!open}
                    title={o.description ?? undefined}
                    onclick={() => togglePick(m, qi, o.label)}
                    >{o.label}{#if o.description}<small>{o.description}</small>{/if}</button
                  >
                {/each}
              </div>
              {#if open}
                <input
                  class="q-other"
                  type="text"
                  placeholder="その他(自由に書く)"
                  bind:value={st[qi].other}
                  onkeydown={(e) => {
                    if (e.key === "Enter" && !e.isComposing) answerQuestion(m);
                  }}
                />
              {/if}
            </div>
          {/each}
          {#if open}
            <div class="ch-row">
              <button class="btn sm primary" type="button" onclick={() => answerQuestion(m)}><Icon name="send" />答えを送る</button>
              <button class="btn sm" type="button" title="決め手は AI が選び、選んだものを報告に書きます" onclick={() => answerQuestion(m, true)}
                >おまかせで進める</button
              >
              <span class="abnote">選ばなかった質問は AI に任せます</span>
            </div>
          {:else}
            <div class="abnote">{m.skipped ? "答えずに次の指示へ進みました" : `答え: ${m.answer}`}</div>
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
  {#if pianoRollStore.focus || soundDesignStore.focus || selectionStore.range || designLabel}
    <div class="chips">
      {#if designTarget}
        <span class="chip" title="設計画面で選んでいる所が指示の対象になります(その所の計画と実際を AI が読み、直すのはその所だけ)"
          ><Icon name="target" size={12} />{designTarget}<button
            class="chip-x"
            onclick={() => (designDetached = designSel.sel)}
            title="この所に限らない指示にする(設計画面の選択はそのまま)"
            aria-label="対象から外す"><Icon name="x" size={11} /></button
          ></span
        >
      {:else if designLabel}
        <button
          class="chip off"
          type="button"
          title="設計画面で選んでいる所を、指示の対象として添え直す"
          onclick={() => (designDetached = null)}><Icon name="target" size={12} />{designLabel} を対象にする</button
        >
      {/if}
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
      {#if designTarget || pianoRollStore.focus || soundDesignStore.focus || selectionStore.range}
        <span class="chips-note">指示はこの対象だけに効きます。ほかの所も頼むときは ✕ で外してください</span>
      {:else}
        <span class="chips-note">指示は曲全体に効きます</span>
      {/if}
    </div>
  {/if}

  <!-- 仕切り(ウィンドウの仕切りの型): ドラッグと上下の矢印で入力欄の高さを変える -->
  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
  <div
    class="input-split"
    role="separator"
    aria-orientation="horizontal"
    aria-label="入力欄の高さ"
    tabindex="0"
    title="ドラッグで入力欄の高さを変える(ダブルクリックで自動に戻す)"
    onpointerdown={startInputResize}
    ondblclick={resetInputH}
    onkeydown={onResizeKey}
  ></div>
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
  .effort,
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

  .msg.abrow {
    align-self: flex-start;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    padding: 2px 4px;
  }
  .msg.abrow .btn.on,
  .abswitch .btn.on {
    border-color: var(--accent);
    color: var(--accent);
  }
  .abnote {
    color: var(--text-faint);
    font-size: 11px;
  }
  .abnote.warn {
    color: var(--warn);
  }
  .msg.question {
    align-self: stretch;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 8px 10px;
    border: 1px solid var(--accent-dim);
    border-radius: 8px;
    background: color-mix(in srgb, var(--accent) 6%, transparent);
  }
  .msg.question.closed {
    border-color: var(--border);
    background: transparent;
  }
  .q-item {
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .q-text {
    font-size: var(--fs-sm);
    color: var(--text);
  }
  .q-tag {
    display: inline-block;
    margin-right: 6px;
    padding: 0 6px;
    border-radius: 4px;
    background: var(--bg-elev, var(--bg-lane));
    color: var(--text-dim);
    font-size: 11px;
  }
  .q-opts {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
  }
  .btn.sm.q-opt {
    height: auto;
    min-height: 24px;
    padding: 3px 9px;
    display: inline-flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 1px;
    text-align: left;
    line-height: 1.35;
  }
  .q-opt small {
    color: var(--text-faint);
    font-size: 10.5px;
    white-space: normal;
  }
  .btn.q-opt.on {
    border-color: var(--accent);
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 14%, transparent);
  }
  .q-other {
    font-size: var(--fs-sm);
    padding: 3px 8px;
  }
  .msg.choices {
    align-self: stretch;
    display: flex;
    flex-direction: column;
    gap: 4px;
    border: 1px solid var(--accent-dim);
    border-radius: 6px;
    padding: 6px 8px;
    font-size: var(--fs-sm);
  }
  .ch-head {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--text-dim);
    font-weight: 600;
  }
  .ch-row {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
  }
  .ch-name {
    font-weight: 600;
  }
  .ch-gone {
    color: var(--text-faint);
    font-size: 11px;
  }
  .abswitch {
    display: inline-flex;
    gap: 4px;
  }
  .spacer {
    flex: 1;
  }
  .pletter {
    min-width: 18px;
    text-align: center;
    border-radius: 3px;
    border: 1px solid var(--accent-dim);
    color: var(--accent);
    font-size: 11px;
    font-weight: 600;
  }
  .pletter.cur {
    background: var(--accent);
    color: var(--bg);
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
  /* 外した対象(押すと添え直す) */
  .chip.off {
    padding: 0 8px;
    border-style: dashed;
    border-color: var(--border-strong);
    background: transparent;
    color: var(--text-dim);
    cursor: pointer;
  }
  .chip.off:hover {
    color: var(--accent);
    border-color: var(--accent-dim);
  }
  .chips-note {
    flex-basis: 100%;
    color: var(--text-faint);
    font-size: 11px;
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

  .input-split {
    height: 5px;
    flex-shrink: 0;
    border-top: 1px solid var(--border);
    cursor: row-resize;
    touch-action: none;
  }

  .input-split:hover,
  .input-split:focus-visible {
    outline: none;
    background: var(--accent-dim);
  }

  .input-row {
    display: flex;
    gap: 6px;
    padding: 4px 10px 10px;
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
