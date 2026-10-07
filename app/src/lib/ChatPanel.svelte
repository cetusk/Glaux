<script lang="ts">
  import Icon from "./Icon.svelte";
  import { onMount, tick, untrack } from "svelte";
  import * as api from "./api";
  import { chatStatus } from "./aiStatus.svelte";
  import { toolShort } from "./toolLabels";
  import { plural, tr } from "./i18n.svelte";
  import { clearAiHighlight, setAiHighlight } from "./aiHighlight.svelte";
  import { renderMarkdown } from "./markdown";
  import { OMAKASE, composePrompt, promptContext, type PromptCtx } from "./chatPrompt";
  import QuestionCard from "./QuestionCard.svelte";
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

  // モデル・考える深さの表示名(settings の表は日本語なので、英語の表示はここで引く)
  const MODEL_TEXT_EN: Record<string, string> = {
    既定: "Default",
    "Claude Code の既定のモデル": "Claude Code's default model",
    "Codex CLI の既定のモデル": "Codex CLI's default model",
    "長い文脈(100 万トークン)": "Long context (1M tokens)",
    速くて軽い: "Fast and light",
    最も高度な作業に: "For the most advanced work",
    普段の作業に: "For everyday work",
    速くて安い: "Fast and cheap",
    前の世代: "Previous generation",
    "前の世代・速い": "Previous generation, fast",
    旧版: "Legacy",
  };
  const EFFORT_LABELS_EN: Record<string, string> = {
    "": "Default",
    minimal: "minimal",
    low: "low (light, fast)",
    medium: "medium (standard)",
    high: "high (careful)",
    xhigh: "xhigh (more careful)",
    max: "max (maximum)",
    ultra: "ultra (deepest, slow)",
  };
  /** モデルの名前・説明の表示(英語の表示では、日本語の所だけ訳す。モデル名はそのまま) */
  function modelText(t: string | undefined): string | undefined {
    if (t == null) return t;
    const en = MODEL_TEXT_EN[t] ?? t.replace("(1M)", " (1M)");
    return tr(t, en);
  }
  function effortLabel(ef: string): string {
    const ja = EFFORT_LABELS[ef] ?? ef;
    return tr(ja, EFFORT_LABELS_EN[ef] ?? ef);
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
    /** role "user": 書いたままの指示と、送ったときの対象(直して送り直すときに使う) */
    raw?: string;
    ctx?: PromptCtx;
    /** role "user": 送ったターンの開始前の最後の履歴エントリと、開始の時刻(ms) */
    sent?: { since: string | null; at: number };
    /** role "user": このターンの会話の終わり(Claude のセッションの位置。巻き戻すときに使う) */
    chainEnd?: string;
  }

  /// 送信待ちの指示(実行中に書いたもの)。会話の下に並べ、前のターンが終わったら順に送る
  interface Pending {
    text: string;
    fullPrompt: string;
    edit?: { raw: string; ctx: PromptCtx };
  }
  let pending = $state<Pending[]>([]);

  /// 実行中のターンの開始前の最後の履歴エントリ(null = 履歴が空)
  let turnStart: string | null = null;

  /// 実行中のターンを始めた時刻と、そのターンで AI が案を出したか
  let turnStartedAt = 0;
  let turnProposed = false;
  /// 実行中のターンを始めた指示(ターンの終わりに会話の位置を書き込む)
  let turnUser: Msg | null = null;

  /// ターンが終わったら、AI の編集の件数を数えて「取り消す」「聴き比べる」を出し、変わった所を縁取る。
  /// 案を出していたら、その案の選択肢(聴き比べる・採用)を並べる
  async function finishTurn() {
    try {
      const ch = await api.turnChanges(turnStart);
      if (ch.entry_ids.length > 0) {
        setAiHighlight(ch);
        push({
          role: "turn",
          text: tr(`このターンの編集 ${ch.entry_ids.length} 件`, `${plural(ch.entry_ids.length, "edit")} in this turn`),
          since: turnStart,
          first: ch.entry_ids[0],
        });
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
      if (!first) throw new Error(tr("このターンの編集が見つかりません", "No edits found in this turn"));
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
        showToast(
          "warn",
          tr(
            "このターンの編集は、今鳴っていないトラック(ミュート中・ほかのトラックのソロ中)だけでした。聴き比べても違いはありません",
            "This turn only edited tracks that aren't sounding (muted, or another track is soloed), so A/B would sound the same",
          ),
        );
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
      showToast("error", tr(`聴き比べを用意できませんでした: ${e}`, `Couldn't prepare the A/B comparison: ${e}`));
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
      showToast(
        "warn",
        tr(
          "曲が変わったので聴き比べを終えました(もう一度「聴き比べる」で聴けます)",
          "The song changed, so A/B was ended (press “Compare” again to listen)",
        ),
      );
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
          ? tr(
              `${r.reverted} 件を取り消しました。後から同じ所を触った編集があります(履歴で確認してください)`,
              `Reverted ${plural(r.reverted, "edit")}. Some later edits touched the same places (check the history)`,
            )
          : tr(
              `このターンの編集 ${r.reverted} 件を取り消しました(Ctrl+Z で取り消しを戻せます)`,
              `Reverted ${plural(r.reverted, "edit")} from this turn (Ctrl+Z brings them back)`,
            ),
      );
    } catch (e) {
      showError(tr("このターンを取り消せませんでした", "Couldn't revert this turn"), e);
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

  /** 会話に足す。足したもの(状態として見張られる版)を返す */
  function push(msg: Msg, force = false): Msg {
    messages.push(msg);
    scrollToBottom(force);
    saveLog();
    return messages[messages.length - 1];
  }

  async function send() {
    const prompt = input.trim();
    if (!prompt) return;
    const ctx = promptContext(designTarget);
    const { shown, fullPrompt } = composePrompt(prompt, ctx);
    rememberPrompt(prompt);
    input = "";
    await sendPrompt(shown, fullPrompt, { raw: prompt, ctx });
  }

  /** 送る(実行中なら送信待ちに並べる)。答えていない質問は「答えずに次へ進んだ」にする */
  async function sendPrompt(shown: string, fullPrompt: string, edit?: { raw: string; ctx: PromptCtx }) {
    for (const m of messages) if (m.role === "question" && !m.answer && !m.skipped) m.skipped = true;
    if (chatStatus.running || pending.length > 0) {
      // 実行中に書いた指示は、今のターンが終わってから送る
      pending.push({ text: shown, fullPrompt, edit });
      scrollToBottom(true);
      return;
    }
    const user = push({ role: "user", text: shown, ...edit }, true);
    await start(fullPrompt, user);
  }

  // ---- AI からの質問(ask_user)----
  /** 質問ごとの選んだ選択肢とその他の言葉(質問の ID → 質問の番号ごと) */
  let qPick = $state<Record<string, { picks: string[]; other: string }[]>>({});
  /** 選んだ内容の入れ物を作る(質問が届いたとき・会話の記録を読んだとき。描画の途中では作らない) */
  function initQuestion(q: api.AiQuestion) {
    if (!qPick[q.id]) qPick[q.id] = q.questions.map(() => ({ picks: [], other: "" }));
  }
  const qState = (m: Msg) => qPick[m.question!.id] ?? m.question!.questions.map(() => ({ picks: [], other: "" }));
  const qOpen = (m: Msg) => !m.answer && !m.skipped;
  /** 答えを送る(`all` なら全部おまかせ)。選ばなかった質問は AI に任せる */
  async function answerQuestion(m: Msg, all = false) {
    if (!qOpen(m)) return;
    const q = m.question!;
    const st = qState(m);
    // 会話に残る答えの行は、返答の言語に合わせる(AI へ送る指示の決まり文句は日本語のまま)
    const en = settings.lang === "en";
    const upToYou = en ? "up to you" : "おまかせ";
    const lines = q.questions.map((qq, i) => {
      const v = all ? [] : [...st[i].picks, ...(st[i].other.trim() ? [st[i].other.trim()] : [])];
      return `- ${qq.header}: ${v.length ? v.join(en ? ", " : "・") : upToYou}`;
    });
    const summary = all ? (en ? "All up to you" : "すべておまかせ") : lines.map((l) => l.slice(2)).join(" / ");
    const shown = `${en ? "Answers: " : "【質問への答え】"}${summary}`;
    const full =
      (all ? OMAKASE : "") +
      `【質問への答え】\n${lines.join("\n")}\nこの答えをもとに、これ以上は質問せずに元の頼みに沿って進める(「計画だけ作って確認を待つ」などの指定があれば守る。「おまかせ」の項目は自分で選び、選んだものを報告に書く)。`;
    // 答えた質問は閉じる(sendPrompt がほかの答えていない質問を「進んだ」にする前に)
    m.answer = summary;
    saveLog();
    await sendPrompt(shown, full);
  }

  /// 送信待ちの次の指示を送る(ターンが終わったとき)
  function sendNext() {
    const next = pending.shift();
    if (!next) return;
    const user = push({ role: "user", text: next.text, ...next.edit }, true);
    start(next.fullPrompt, user);
  }

  async function start(fullPrompt: string, user: Msg) {
    chatStatus.running = true;
    clearAiHighlight();
    turnStartedAt = Date.now();
    turnProposed = false;
    turnUser = user;
    try {
      const h = await api.getHistory(1);
      turnStart = h.entries.length > 0 ? h.entries[h.entries.length - 1].id : null;
      user.sent = { since: turnStart, at: turnStartedAt };
      saveLog();
    } catch {
      turnStart = null;
    }
    try {
      await api.sendChat(fullPrompt, currentModel, provider, currentEffort, settings.lang);
    } catch (e) {
      push({ role: "error", text: String(e) });
      chatStatus.running = false;
    }
  }

  // ---- 送った指示を直して送り直す(その指示より後の編集を人の分も戻し、会話もその前まで戻す) ----
  let editing = $state<{
    m: Msg;
    text: string;
    preview: { song: number; plan: number; human: number } | null;
    busy: boolean;
    error: string | null;
  } | null>(null);
  let editEl = $state<HTMLTextAreaElement | undefined>();
  const canEdit = (m: Msg) => m.role === "user" && !m.dropped && m.raw != null && m.sent != null;

  async function beginEdit(m: Msg) {
    if (chatStatus.running || pending.length > 0 || !canEdit(m)) return;
    editing = { m, text: m.raw!, preview: null, busy: false, error: null };
    await tick();
    editEl?.focus();
    try {
      const p = await api.chatRewindPreview(m.sent!.since, m.sent!.at);
      if (editing?.m === m) editing.preview = p;
    } catch (e) {
      if (editing?.m === m) editing.error = String(e);
    }
  }

  async function resendEdited() {
    const ed = editing;
    if (!ed || ed.busy || chatStatus.running) return;
    const text = ed.text.trim();
    const idx = messages.indexOf(ed.m);
    if (!text || idx < 0 || !ed.m.sent) return;
    // 会話は一つ前の指示のターンの終わりから続ける(無ければ新しい会話)
    let prev: Msg | undefined;
    for (let i = idx - 1; i >= 0 && !prev; i--) if (messages[i].role === "user" && messages[i].sent) prev = messages[i];
    const chainEnd = provider === "claude" ? (prev?.chainEnd ?? null) : null;
    const ctx = ed.m.ctx ?? { prefix: "", deco: "" };
    ed.busy = true;
    ed.error = null;
    try {
      await api.chatRewind(ed.m.sent.since, ed.m.sent.at, chainEnd);
    } catch (e) {
      ed.busy = false;
      ed.error = String(e);
      return;
    }
    editing = null;
    clearAiHighlight();
    messages.splice(idx);
    if (prev && !chainEnd) {
      push({
        role: "notice",
        text:
          provider === "claude"
            ? tr(
                "前のやりとりの終わりの位置が分からないため、新しい会話として送ります(曲と計画は戻してあります)",
                "Couldn't find where the previous exchange ended, so this is sent as a new conversation (the song and plan are rewound)",
              )
            : tr(
                "GPT では会話を途中まで戻せないため、新しい会話として送ります(曲と計画は戻してあります)",
                "GPT can't rewind a conversation partway, so this is sent as a new conversation (the song and plan are rewound)",
              ),
      });
    }
    saveLog();
    const { shown, fullPrompt } = composePrompt(text, ctx);
    rememberPrompt(text);
    await sendPrompt(shown, fullPrompt, { raw: text, ctx });
  }

  function onEditKeydown(e: KeyboardEvent) {
    if (e.isComposing) return;
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void resendEdited();
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      editing = null;
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
            if (turnUser && ev.chain_end) {
              turnUser.chainEnd = ev.chain_end;
              saveLog();
            }
            turnUser = null;
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
            turnUser = null;
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
    <h2><Icon name="sparkles" size={15} />{tr("AI に指示", "AI assistant")}</h2>
    <div class="head-right">
      <select
        class="provider"
        value={provider}
        onchange={pickProvider}
        disabled={chatStatus.running}
        title={tr(
          "チャットの相手(Claude = Claude Code、GPT = Codex CLI。どちらもインストールしてログインしておく)。切り替えると新しい会話になります",
          "Who to chat with (Claude = Claude Code, GPT = Codex CLI; install and sign in to either first). Switching starts a new conversation",
        )}
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
        title={tr("AI のモデル(次の指示から反映。会話の文脈はそのまま引き継がれます)", "AI model (applies from the next message; the conversation context is kept)")}
      >
        {#each MODELS[provider] as m (m.value)}
          <option value={m.value} title={modelText(m.note)}>{modelText(m.label)}</option>
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
        title={tr(
          "考える深さ(effort)。深いほど丁寧だが時間と使用量が増える。次の指示から反映",
          "Reasoning effort. Deeper is more careful but takes more time and usage. Applies from the next message",
        )}
        aria-label={tr("考える深さ", "Reasoning effort")}
      >
        <option value="">{tr("深さ: 既定", "Effort: default")}</option>
        {#each efforts as ef (ef)}
          <option value={ef}>{effortLabel(ef)}</option>
        {/each}
      </select>
      <button class="btn sm" onclick={newConversation} disabled={chatStatus.running} title={tr("会話の文脈をリセットする(表示中の会話も消えます)", "Reset the conversation context (also clears the shown conversation)")}
        ><Icon name="message-square-plus" />{tr("新しい会話", "New chat")}</button
      >
    </div>
  </div>

  <div class="messages" bind:this={scroller} onscroll={onScroll}>
    {#if messages.length === 0}
      <div class="hint">
        {tr(
          "例:「4小節のベースラインを作って」「もっと音を明るくして」「さっきの編集を取り消して」",
          "e.g. “Write a 4-bar bassline”, “Make it brighter”, “Undo that last edit”",
        )}
      </div>
    {/if}
    {#each messages as m}
      {#if m.role === "tool"}
        <div class="msg tool"><Icon name="sparkles" size={12} />{m.text}</div>
      {:else if m.role === "turn"}
        <div class="msg turn">
          <span>{m.text}{m.reverted ? tr("(取り消し済み)", " (reverted)") : ""}</span>
          {#if !m.reverted}
            <button
              class="btn sm"
              onclick={() => revertTurnAt(m)}
              title={tr(
                "このターンで AI が行った編集をまとめて打ち消す(後から人間が行った編集は残す)",
                "Undo all of the AI's edits from this turn (later edits by you are kept)",
              )}
              ><Icon name="eraser" />{tr("このターンを取り消す", "Revert this turn")}</button
            >
            {#if turnAb?.key !== turnKey(m)}
              <button
                class="btn sm"
                disabled={turnAbBusy != null}
                onclick={() => startTurnAb(m)}
                title={tr(
                  "このターンの前と今を、音量をそろえて切り替えて聴く(範囲はこのターンで音が変わった所)",
                  "Switch between before and after this turn at matched loudness (over the range where the sound changed)",
                )}
                ><Icon name="headphones" />{turnAbBusy === turnKey(m)
                  ? tr("用意しています…", "Preparing…")
                  : tr("前と今を聴き比べる", "Compare before/after")}</button
              >
            {/if}
          {/if}
        </div>
        {#if turnAb && turnAb.key === turnKey(m)}
          <div class="msg abrow" data-ab-live role="group" aria-label={tr("このターンの前と今を切り替える", "Switch between before and after this turn")}>
            <button class="btn sm" class:on={turnAb.side === "a"} type="button" onclick={() => setTurnSide("a")}>{tr("A 前", "A Before")}</button>
            <button class="btn sm" class:on={turnAb.side === "b"} type="button" onclick={() => setTurnSide("b")}>{tr("B 今", "B Now")}</button>
            <button class="btn sm" type="button" title={tr("範囲の頭から聴き直す", "Play again from the start of the range")} onclick={() => turnAb && restartAb(turnAb.start)}
              >⏮ {tr("頭から", "Restart")}</button
            >
            <button class="btn sm" type="button" onclick={endTurnAb}>{tr("終える", "End")}</button>
            {#if turnAb.info.first_diff_secs == null}
              <span class="abnote warn"
                >{tr(
                  "この範囲では、前と今の音が同じです。このターンの編集は、ミュートしたトラックなど今は聞こえない所だけかもしれません",
                  "Before and after sound the same in this range. This turn may only have edited parts you can't hear now, such as muted tracks",
                )}</span
              >
            {:else}
              <span class="abnote"
                >{tr(
                  "音量はそろえてあります。切り替えても同じ位置から続けて鳴ります",
                  "Loudness is matched. Switching keeps playing from the same position",
                )}</span
              >
            {/if}
          </div>
        {/if}
      {:else if m.role === "choices"}
        {@const live = liveProposals(m.planIds)}
        {@const on = comparing(live)}
        <div class="msg choices" data-ab-live={on ? "" : undefined}>
          <div class="ch-head"><Icon name="split" size={13} />{tr(`AI の案(${m.planIds?.length ?? 0})`, `AI proposals (${m.planIds?.length ?? 0})`)}</div>
          {#each m.planIds ?? [] as id (id)}
            {@const k = live.indexOf(id)}
            <div class="ch-row">
              {#if k >= 0}
                <span class="pletter" class:cur={on && proposalAb.side === proposalAb.planIds.indexOf(id) + 1}>{abLetter(k + 1)}</span>
                <span class="ch-name">{planName(id)}</span>
                <span class="spacer"></span>
                <button class="btn sm primary" type="button" title={tr(
                    "案の音を曲に当て、案の計画を今の計画にする(同じ頼みのほかの案は片付ける)",
                    "Apply this proposal to the song and make its plan current (other proposals for the same request are discarded)",
                  )}
                  onclick={() => adoptProposal(id, planName(id) ?? tr("案", "proposal"))}
                  >{tr("採用", "Adopt")}</button
                >
              {:else}
                <span class="ch-gone">{tr("採用したか、片付けた案", "Adopted or discarded")}</span>
              {/if}
            </div>
          {/each}
          {#if live.length}
            <div class="ch-row">
              {#if on}
                <span class="abswitch" role="group" aria-label={tr("今と案を切り替える", "Switch between now and the proposals")}>
                  <button class="btn sm" class:on={proposalAb.side === 0} type="button" onclick={() => setProposalSide(0)}>{tr("A 今", "A Now")}</button>
                  {#each proposalAb.planIds as pid, j (pid)}
                    <button class="btn sm" class:on={proposalAb.side === j + 1} type="button" title={planName(pid)} onclick={() => setProposalSide(j + 1)}
                      >{abLetter(j + 1)}</button
                    >
                  {/each}
                </span>
                <button class="btn sm" type="button" title={tr("範囲の頭から聴き直す", "Play again from the start of the range")} onclick={restartProposalAb}
                  >⏮ {tr("頭から", "Restart")}</button
                >
                <button class="btn sm" type="button" onclick={() => endProposalAb()}>{tr("終える", "End")}</button>
              {:else}
                <button class="btn sm" type="button" disabled={proposalAb.busy != null} onclick={() => listenProposals(live)}
                  ><Icon name="headphones" />{live.length >= 2
                    ? tr(
                        `まとめて聴き比べる(今 + 案 ${Math.min(live.length, MAX_PROPOSALS_AB)} つ)`,
                        `Compare all (now + ${plural(Math.min(live.length, MAX_PROPOSALS_AB), "proposal")})`,
                      )
                    : tr("今と聴き比べる", "Compare with now")}</button
                >
              {/if}
              <button class="btn sm" type="button" onclick={() => (viewStore.main = "design")}>{tr("設計画面で見る", "View in Design")}</button>
            </div>
            {#if on}<div class="abnote">{tr(
                  "音量はいちばん小さいものにそろえてあります。切り替えても同じ位置から続けて鳴ります",
                  "Loudness is matched to the quietest one. Switching keeps playing from the same position",
                )}</div>{/if}
          {:else}
            <div class="abnote">{tr("どの案も、もう残っていません", "No proposals are left")}</div>
          {/if}
        </div>
      {:else if m.role === "question" && m.question}
        <QuestionCard question={m.question} answer={m.answer} skipped={m.skipped} open={qOpen(m)} st={qState(m)} onAnswer={(all) => answerQuestion(m, all)} />
      {:else if m.role === "assistant"}
        <div class="msg assistant md">{@html renderMarkdown(m.text)}</div>
      {:else if m.role === "user" && m.dropped}
        <div class="msg user dropped">
          <span class="struck">{m.text}</span>
          <div class="queue-note">{tr("停止したので送っていません", "Not sent (stopped)")}</div>
        </div>
      {:else if m.role === "user" && editing?.m === m}
        <div class="msg user editing">
          <textarea
            bind:this={editEl}
            bind:value={editing.text}
            rows="3"
            disabled={editing.busy}
            aria-label={tr("直した指示", "Edited message")}
            onkeydown={onEditKeydown}
          ></textarea>
          <div class="edit-note">
            {#if editing.error}
              <span class="edit-err">{editing.error}</span>
            {:else if !editing.preview}
              {tr("この指示より後の編集を数えています…", "Counting the edits after this message…")}
            {:else if editing.preview.song + editing.preview.plan > 0}
              {tr(
                `この指示より後の編集 ${editing.preview.song + editing.preview.plan} 件` +
                  (editing.preview.human ? `(うち人の編集 ${editing.preview.human} 件)` : "") +
                  "を取り消し、会話もこの指示の前まで戻してから送ります。取り消しは履歴に残ります",
                `Undoes ${plural(editing.preview.song + editing.preview.plan, "edit")} made after this message` +
                  (editing.preview.human ? ` (${editing.preview.human} by you)` : "") +
                  ", rewinds the conversation to before it, then sends. The undo stays in the history",
              )}
            {:else}
              {tr(
                "この指示より後の編集はありません。会話をこの指示の前まで戻してから送ります",
                "No edits since this message. Rewinds the conversation to before it, then sends",
              )}
            {/if}
          </div>
          <div class="ch-row">
            <button class="btn sm primary" type="button" disabled={editing.busy || !editing.text.trim()} onclick={resendEdited}
              ><Icon name="send" />{tr("戻して送り直す", "Rewind and resend")}</button
            >
            <button class="btn sm" type="button" disabled={editing.busy} onclick={() => (editing = null)}>{tr("やめる", "Cancel")}</button>
          </div>
        </div>
      {:else if m.role === "user" && canEdit(m)}
        <div class="msg user editable">
          {m.text}<button
            class="edit-btn"
            type="button"
            disabled={chatStatus.running || pending.length > 0 || editing != null}
            title={chatStatus.running || pending.length > 0
              ? tr("AI の作業が終わってから直せます", "You can edit after the AI finishes")
              : tr(
                  "この指示を直して送り直す(この指示より後の編集と会話を戻します)",
                  "Edit and resend this message (rewinds later edits and the conversation)",
                )}
            aria-label={tr("指示を直す", "Edit message")}
            onclick={() => beginEdit(m)}><Icon name="pencil" size={12} /></button
          >
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
          {tr("送信待ち(今の指示が終わったら送ります)", "Queued (sends when the current message finishes)")}
          <button
            class="chip-x"
            onclick={() => dropPending(i)}
            title={tr("この指示を送らない", "Don't send this message")}
            aria-label={tr("この指示を送らない", "Don't send this message")}
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
        <span
          class="chip"
          title={tr(
            "設計画面で選んでいる所が指示の対象になります(その所の計画と実際を AI が読み、直すのはその所だけ)",
            "Messages target the part selected in Design (the AI reads its plan and actual content, and only changes that part)",
          )}
          ><Icon name="target" size={12} />{designTarget}<button
            class="chip-x"
            onclick={() => (designDetached = designSel.sel)}
            title={tr("この所に限らない指示にする(設計画面の選択はそのまま)", "Don't limit messages to this part (the Design selection is kept)")}
            aria-label={tr("対象から外す", "Remove target")}><Icon name="x" size={11} /></button
          ></span
        >
      {:else if designLabel}
        <button
          class="chip off"
          type="button"
          title={tr("設計画面で選んでいる所を、指示の対象として添え直す", "Target the part selected in Design again")}
          onclick={() => (designDetached = null)}
          ><Icon name="target" size={12} />{tr(`${designLabel} を対象にする`, `Target ${designLabel}`)}</button
        >
      {/if}
      {#if pianoRollStore.focus}
        <span class="chip" title={tr("ピアノロールで開いているクリップが指示の対象になります", "Messages target the clip open in the piano roll")}
          ><Icon name="piano" size={12} />{pianoRollStore.focus.clipName}<button
            class="chip-x"
            onclick={() => (pianoRollStore.focus = null)}
            title={tr("ピアノロールを閉じる", "Close the piano roll")}
            aria-label={tr("ピアノロールを閉じる", "Close the piano roll")}><Icon name="x" size={11} /></button
          ></span
        >
      {/if}
      {#if soundDesignStore.focus}
        <span
          class="chip"
          title={soundDesignStore.focus.trackId === MASTER_FOCUS_ID
            ? tr("エフェクトの指示はマスターへ", "Effect requests go to the master")
            : tr("音色・エフェクトの指示はこのトラックへ", "Sound and effect requests go to this track")}
          ><Icon name="sliders-horizontal" size={12} />{tr(`${soundDesignStore.focus.trackName} の音作り`, `Sound design: ${soundDesignStore.focus.trackName}`)}<button
            class="chip-x"
            onclick={() => (soundDesignStore.focus = null)}
            title={tr("インスペクターを閉じる", "Close the inspector")}
            aria-label={tr("インスペクターを閉じる", "Close the inspector")}><Icon name="x" size={11} /></button
          ></span
        >
      {/if}
      {#if selectionStore.range}
        <span class="chip" title={tr("この範囲に限定して指示されます", "Messages are limited to this range")}
          ><Icon name="ruler" size={12} />{tr(
            `小節 ${selectionStore.range.startBar + 1}〜${selectionStore.range.endBar + 1}`,
            `Bars ${selectionStore.range.startBar + 1}–${selectionStore.range.endBar + 1}`,
          )}<button
            class="chip-x"
            onclick={() => (selectionStore.range = null)}
            title={tr("範囲の指定を外す", "Clear the range")}
            aria-label={tr("範囲の指定を外す", "Clear the range")}><Icon name="x" size={11} /></button
          ></span
        >
      {/if}
      {#if designTarget || pianoRollStore.focus || soundDesignStore.focus || selectionStore.range}
        <span class="chips-note"
          >{tr(
            "指示はこの対象だけに効きます。ほかの所も頼むときは ✕ で外してください",
            "Messages only affect this target. Remove it with ✕ to work on other parts",
          )}</span
        >
      {:else}
        <span class="chips-note">{tr("指示は曲全体に効きます", "Messages affect the whole song")}</span>
      {/if}
    </div>
  {/if}

  <!-- 仕切り(ウィンドウの仕切りの型): ドラッグと上下の矢印で入力欄の高さを変える -->
  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
  <div
    class="input-split"
    role="separator"
    aria-orientation="horizontal"
    aria-label={tr("入力欄の高さ", "Input height")}
    tabindex="0"
    title={tr("ドラッグで入力欄の高さを変える(ダブルクリックで自動に戻す)", "Drag to resize the input (double-click to reset to auto)")}
    onpointerdown={startInputResize}
    ondblclick={resetInputH}
    onkeydown={onResizeKey}
  ></div>
  <div class="input-row">
    <textarea
      bind:this={inputEl}
      rows="2"
      placeholder={chatStatus.running
        ? tr("次の指示を書けます(今の指示が終わったら送ります)", "You can write the next message (it sends when the current one finishes)")
        : tr(
            "AI への指示を入力(Enter で送信 / Shift+Enter で改行 / ↑ で前の指示)",
            "Message the AI (Enter to send / Shift+Enter for a new line / ↑ for previous)",
          )}
      bind:value={input}
      onkeydown={onKeydown}
    ></textarea>
    {#if chatStatus.running}
      {#if input.trim()}
        <button class="send" onclick={send} title={tr("今の指示が終わったら送ります", "Sends when the current message finishes")}
          >{tr("予約", "Queue")}</button
        >
      {/if}
      <button class="stop" onclick={cancel} title={tr("実行中の指示を中断する(送信待ちの指示も送りません)", "Stop the running message (queued messages won't be sent either)")}
        >{tr("停止", "Stop")}</button
      >
    {:else}
      <button class="send" onclick={send} disabled={!input.trim()}>{tr("送信", "Send")}</button>
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

  .msg.user.editable {
    position: relative;
  }

  .edit-btn {
    position: absolute;
    left: -24px;
    top: 4px;
    display: inline-flex;
    padding: 3px;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: var(--text-dim);
    cursor: pointer;
    opacity: 0;
    transition: opacity 0.12s;
  }

  .msg.user.editable:hover .edit-btn,
  .edit-btn:focus-visible {
    opacity: 1;
  }

  .edit-btn:hover:not(:disabled) {
    color: var(--text);
    background: var(--bg-hover, color-mix(in srgb, var(--text) 10%, transparent));
  }

  .edit-btn:disabled {
    cursor: default;
    opacity: 0;
  }

  .msg.user.editable:hover .edit-btn:disabled {
    opacity: 0.35;
  }

  .msg.user.editing {
    align-self: stretch;
    max-width: none;
    display: flex;
    flex-direction: column;
    gap: 6px;
    white-space: normal;
  }

  .msg.user.editing textarea {
    width: 100%;
    box-sizing: border-box;
    resize: vertical;
    font: inherit;
    font-size: 13px;
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg);
    color: var(--text);
  }

  .edit-note {
    font-size: var(--fs-xs);
    color: var(--text-dim);
  }

  .edit-err {
    color: var(--danger, #e5534b);
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
