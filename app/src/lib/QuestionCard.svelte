<script lang="ts">
  // AI からの質問(ask_user)のカード: 質問ごとに選択肢を選ぶ(いくつでもの質問は複数)かその他を書いて答えを送る。
  // 選んだ内容(st)は会話の並びが変わっても失わないよう、チャットが質問の ID ごとに持つ
  import Icon from "./Icon.svelte";
  import type * as api from "./api";
  import { tr } from "./i18n.svelte";

  let {
    question,
    answer,
    skipped,
    open,
    st,
    onAnswer,
  }: {
    question: api.AiQuestion;
    answer?: string;
    skipped?: boolean;
    /** まだ答えていない(答えずに次へ進んでもいない) */
    open: boolean;
    /** 質問ごとの選んだ選択肢とその他の言葉 */
    st: { picks: string[]; other: string }[];
    /** 答えを送る(all = 全部おまかせ) */
    onAnswer: (all: boolean) => void;
  } = $props();

  function togglePick(qi: number, label: string) {
    const s = st[qi];
    s.picks = s.picks.includes(label) ? s.picks.filter((x) => x !== label) : question.questions[qi].multi ? [...s.picks, label] : [label];
  }
</script>

<div class="msg question" class:closed={!open} role="group" aria-label={tr("AI からの質問", "Question from the AI")}>
  <div class="ch-head"><Icon name="message-circle-question-mark" size={13} />{tr("AI からの質問", "Question from the AI")}{question.why
      ? tr(`(${question.why})`, ` (${question.why})`)
      : ""}</div
  >
  {#each question.questions as q, qi (qi)}
    <div class="q-item">
      <div class="q-text"><span class="q-tag">{q.header}</span>{q.question}{q.multi ? tr("(いくつでも)", " (pick any)") : ""}</div>
      <div class="q-opts">
        {#each q.options as o (o.label)}
          <button
            class="btn sm q-opt"
            class:on={st[qi].picks.includes(o.label)}
            type="button"
            disabled={!open}
            title={o.description ?? undefined}
            onclick={() => togglePick(qi, o.label)}
            >{o.label}{#if o.description}<small>{o.description}</small>{/if}</button
          >
        {/each}
      </div>
      {#if open}
        <input
          class="q-other"
          type="text"
          placeholder={tr("その他(自由に書く)", "Other (write freely)")}
          bind:value={st[qi].other}
          onkeydown={(e) => {
            if (e.key === "Enter" && !e.isComposing) onAnswer(false);
          }}
        />
      {/if}
    </div>
  {/each}
  {#if open}
    <div class="ch-row">
      <button class="btn sm primary" type="button" onclick={() => onAnswer(false)}><Icon name="send" />{tr("答えを送る", "Send answers")}</button>
      <button class="btn sm" type="button" title={tr("決め手は AI が選び、選んだものを報告に書きます", "The AI decides and reports what it chose")}
        onclick={() => onAnswer(true)}>{tr("おまかせで進める", "Leave it to AI")}</button
      >
      <span class="abnote">{tr("選ばなかった質問は AI に任せます", "Unanswered questions are left to the AI")}</span>
    </div>
  {:else}
    <div class="abnote">{skipped
        ? tr("答えずに次の指示へ進みました", "Moved on to the next message without answering")
        : tr(`答え: ${answer}`, `Answer: ${answer}`)}</div>
  {/if}
</div>

<style>
  .msg {
    border-radius: 8px;
    padding: 7px 10px;
    font-size: 13px;
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-word;
    max-width: 95%;
  }

  .abnote {
    color: var(--text-faint);
    font-size: 11px;
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
</style>
