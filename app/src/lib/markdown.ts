// チャットの AI の返答を表示するための小さな Markdown 変換。
// 最初に HTML を全部エスケープしてから、見出し・箇条書き・表・コード・強調だけを HTML にする
// (AI の出力をそのまま HTML として解釈しない。リンクは文字のまま = アプリ内で外へ飛ばない)。

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

/** 1 行の中の強調・コード(エスケープ済みの文字列に対して) */
function inline(escaped: string): string {
  // コードの中身は強調の対象にしないので、先に取り出して後で戻す
  const codes: string[] = [];
  let s = escaped.replace(/`([^`]+)`/g, (_, c: string) => {
    codes.push(c);
    return `\u0000${codes.length - 1}\u0000`;
  });
  s = s
    .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
    .replace(/__([^_]+)__/g, "<strong>$1</strong>")
    .replace(/(^|[^*])\*([^*\s][^*]*)\*/g, "$1<em>$2</em>")
    .replace(/~~([^~]+)~~/g, "<del>$1</del>")
    // [文字](URL) は文字だけ残す
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1");
  return s.replace(/\u0000(\d+)\u0000/g, (_, i: string) => `<code>${codes[Number(i)]}</code>`);
}

function splitRow(line: string): string[] {
  let t = line.trim();
  if (t.startsWith("|")) t = t.slice(1);
  if (t.endsWith("|")) t = t.slice(0, -1);
  return t.split("|").map((c) => c.trim());
}

const TABLE_SEP = /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/;

/** Markdown を安全な HTML にする */
export function renderMarkdown(src: string): string {
  const lines = src.replace(/\r\n?/g, "\n").split("\n");
  const out: string[] = [];
  let para: string[] = [];
  const flush = () => {
    if (para.length > 0) {
      out.push(`<p>${para.map((l) => inline(escapeHtml(l))).join("<br>")}</p>`);
      para = [];
    }
  };
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    // コードのかたまり
    const fence = line.match(/^\s*(```|~~~)/);
    if (fence) {
      flush();
      const body: string[] = [];
      i++;
      while (i < lines.length && !lines[i].trim().startsWith(fence[1])) {
        body.push(lines[i]);
        i++;
      }
      i++;
      out.push(`<pre><code>${escapeHtml(body.join("\n"))}</code></pre>`);
      continue;
    }
    if (line.trim() === "") {
      flush();
      i++;
      continue;
    }
    const heading = line.match(/^(#{1,6})\s+(.*)$/);
    if (heading) {
      flush();
      // 見出しはチャットの中では大きくしすぎない(h4〜h6 だけを使う)
      const level = Math.min(6, heading[1].length + 3);
      out.push(`<h${level}>${inline(escapeHtml(heading[2]))}</h${level}>`);
      i++;
      continue;
    }
    if (/^\s*([-*_])\s*(\1\s*){2,}$/.test(line)) {
      flush();
      out.push("<hr>");
      i++;
      continue;
    }
    // 表(見出し行 + 区切り行)
    if (line.includes("|") && i + 1 < lines.length && TABLE_SEP.test(lines[i + 1])) {
      flush();
      const head = splitRow(line);
      i += 2;
      const rows: string[][] = [];
      while (i < lines.length && lines[i].includes("|") && lines[i].trim() !== "") {
        rows.push(splitRow(lines[i]));
        i++;
      }
      const th = head.map((c) => `<th>${inline(escapeHtml(c))}</th>`).join("");
      const tb = rows
        .map((r) => `<tr>${head.map((_, k) => `<td>${inline(escapeHtml(r[k] ?? ""))}</td>`).join("")}</tr>`)
        .join("");
      out.push(`<div class="md-table"><table><thead><tr>${th}</tr></thead><tbody>${tb}</tbody></table></div>`);
      continue;
    }
    if (/^>\s?/.test(line)) {
      flush();
      const body: string[] = [];
      while (i < lines.length && /^>\s?/.test(lines[i])) {
        body.push(lines[i].replace(/^>\s?/, ""));
        i++;
      }
      out.push(`<blockquote>${renderMarkdown(body.join("\n"))}</blockquote>`);
      continue;
    }
    // 箇条書き(入れ子は字下げで 1 段だけ見分ける)
    const item = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/;
    if (item.test(line)) {
      flush();
      const ordered = /\d/.test(line.match(item)![2]);
      const items: string[] = [];
      while (i < lines.length && item.test(lines[i])) {
        const m = lines[i].match(item)!;
        const nested = m[1].length >= 2;
        const text = inline(escapeHtml(m[3]));
        items.push(nested ? `<li class="nested">${text}</li>` : `<li>${text}</li>`);
        i++;
        // 続きの行(字下げされた、印のない行)は同じ項目に足す
        while (i < lines.length && /^\s{2,}\S/.test(lines[i]) && !item.test(lines[i])) {
          items[items.length - 1] = items[items.length - 1].replace(
            /<\/li>$/,
            `<br>${inline(escapeHtml(lines[i].trim()))}</li>`,
          );
          i++;
        }
      }
      const tag = ordered ? "ol" : "ul";
      out.push(`<${tag}>${items.join("")}</${tag}>`);
      continue;
    }
    para.push(line);
    i++;
  }
  flush();
  return out.join("");
}
