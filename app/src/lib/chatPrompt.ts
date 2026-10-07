// チャットの指示に添える前置きを組み立てる: 今選んでいる対象(ピアノロールのクリップ・小節の範囲・設計画面の所・
// 音作りのトラック)と、/goal・設定「途中で尋ねない」の「おまかせ」
import { designSel, designStore, designTargetPrompt } from "./design.svelte";
import { tr } from "./i18n.svelte";
import { MASTER_FOCUS_ID, pianoRollStore, selectionStore, soundDesignStore } from "./selection.svelte";
import { settings } from "./settings.svelte";

/// 指示に添える対象(範囲・クリップ・設計の所・音作り): AI へ送る前置きと、会話に出す頭書き
export interface PromptCtx {
  prefix: string;
  deco: string;
}

/** 質問せずに最後まで作らせる前置き(/goal・設定「途中で尋ねない」・質問のカードの「おまかせで進める」) */
export const OMAKASE =
  "【おまかせ】ask_user で質問せず、決め手(ジャンル・雰囲気・長さ・編成など)は自分で選んで最後まで作る。選んだ決め手は報告に書く。\n";
const GOAL_DEFAULT_JA = "何でもよいので、1 曲作ってください(ジャンル・雰囲気・長さ・編成は自由に選んでください)。";
const GOAL_DEFAULT_EN = "Make any song you like (choose the genre, mood, length and instruments freely).";

/** 今選んでいる対象を指示の前置きにする(タイムラインの小節範囲・ピアノロールで開いているクリップなど = マスク) */
export function promptContext(designTarget: string | null): PromptCtx {
  const range = selectionStore.range;
  const focus = pianoRollStore.focus;
  let prefix = "";
  let shown = "";
  if (focus) {
    prefix +=
      `【対象クリップ(ユーザーがピアノロールで開いている)】トラック「${focus.trackName}」(${focus.trackId})の` +
      `クリップ「${focus.clipName}」(${focus.clipId})。特に指定がなければ、このクリップ内のノートへの操作として解釈してください。\n`;
    shown = tr(`〔${focus.clipName}〕 ${shown}`, `[${focus.clipName}] ${shown}`);
  }
  if (range) {
    prefix +=
      `【対象範囲の指定(ユーザーが UI で選択)】小節 ${range.startBar + 1}〜${range.endBar + 1}` +
      `(tick ${range.startTick}〜${range.endTick})。` +
      `編集・分析はこの範囲内に限定し、範囲外のノートやクリップは変更しないでください。\n`;
    shown = tr(
      `〔小節 ${range.startBar + 1}〜${range.endBar + 1}〕 ${shown}`,
      `[Bars ${range.startBar + 1}–${range.endBar + 1}] ${shown}`,
    );
  }
  if (designTarget) {
    prefix += designTargetPrompt(designStore.data, designSel.sel) ?? "";
    shown = tr(`〔${designTarget}〕 ${shown}`, `[${designTarget}] ${shown}`);
  }
  const sd = soundDesignStore.focus;
  if (sd && sd.trackId === MASTER_FOCUS_ID) {
    prefix +=
      "【音作り中: マスターバス(ユーザーがマスターのエフェクトを開いている)】" +
      "エフェクトに関する指示は、特に指定がなければマスター(add_master_effect / set_master_param)が対象です。\n";
    shown = tr(`〔音作り: マスター〕 ${shown}`, `[Sound design: Master] ${shown}`);
  } else if (sd) {
    prefix +=
      `【音作り中のトラック(ユーザーが音作りビューで開いている)】「${sd.trackName}」(${sd.trackId})。` +
      `音色・エフェクトに関する指示は、特に指定がなければこのトラックが対象です。\n`;
    shown = tr(`〔音作り: ${sd.trackName}〕 ${shown}`, `[Sound design: ${sd.trackName}] ${shown}`);
  }
  return { prefix, deco: shown };
}

/** 書いた指示と対象から、会話に出す文と AI へ送る文を作る */
export function composePrompt(prompt: string, ctx: PromptCtx): { shown: string; fullPrompt: string } {
  let prefix = ctx.prefix;
  let shown = ctx.deco + prompt;
  // /goal: 質問せずに AI が決め手を選んで最後まで作る(何も無ければ、何でもよいので 1 曲)。設定で「途中で尋ねない」なら毎回
  const goal = prompt.match(/^\/(goal|おまかせ)(?:\s+|$)([\s\S]*)$/);
  let body = prompt;
  if (goal) {
    body = goal[2].trim() || (settings.lang === "en" ? GOAL_DEFAULT_EN : GOAL_DEFAULT_JA);
    shown = `${ctx.deco}${tr("〔おまかせ〕", "[Up to you]")} ${body}`;
  }
  if (goal || settings.chatAsk === "never") prefix = OMAKASE + prefix;
  return { shown, fullPrompt: prefix ? `${prefix}\n${body}` : body };
}

