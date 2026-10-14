// マウスを載せたときの説明(吹き出し)。ブラウザ標準の title の吹き出しは、出るまでに 0.5〜1 秒ほど待ち、
// 少し動くと出直しになり、押した直後やボタンからボタンへ移ったときには出ないことがあった。
// 画面全体で 1 か所: マウスの下の要素の title を読み取り(標準の吹き出しを出さないよう、載っている間は title を外して
// data-tip に移す)、少し止まったら Glaux の吹き出しを出す。一度出た後は、隣へ移るとすぐ出す。
// 各部品は今までどおり title を書けばよい(載っている間に title が書き換わっても読み直す)

const DELAY_MS = 300;
/** 出ていた吹き出しが消えてから、この間に次へ載せたらすぐ出す */
const WARM_MS = 600;
const GAP = 6;

let tip: HTMLDivElement | null = null;
let current: Element | null = null;
let timer: ReturnType<typeof setTimeout> | undefined;
let lastHidden = 0;
let shown = false;

/** マウスの下で、説明を持つ一番近い要素(title か、外した後の data-tip) */
function tipOwner(el: Element | null): Element | null {
  for (let e = el; e && e !== document.body; e = e.parentElement) {
    // SVG の中の <title> は SVG の決まりで出す(ここでは扱わない)
    if (e.hasAttribute("title") || e.hasAttribute("data-tip")) return e;
  }
  return null;
}

/** title を data-tip に移す(標準の吹き出しを出さない)。読み上げ用の名前が無ければ、その文を名前にする */
function adopt(e: Element): string {
  const t = e.getAttribute("title");
  if (t != null) {
    e.removeAttribute("title");
    if (t) e.setAttribute("data-tip", t);
    else e.removeAttribute("data-tip");
    if (t && !e.hasAttribute("aria-label") && !(e.textContent ?? "").trim()) e.setAttribute("aria-label", t);
  }
  return e.getAttribute("data-tip") ?? "";
}

/** 離れたら title に戻す(部品の側の書き換え・外し〈title={条件 ? 文 : undefined}〉がそのまま効くように) */
function release(e: Element | null) {
  if (!e) return;
  const t = e.getAttribute("data-tip");
  if (t != null && !e.hasAttribute("title")) e.setAttribute("title", t);
  e.removeAttribute("data-tip");
}

function hide() {
  clearTimeout(timer);
  if (shown) lastHidden = Date.now();
  shown = false;
  release(current);
  current = null;
  if (tip) tip.style.display = "none";
}

function place(owner: Element, text: string) {
  if (!tip) return;
  tip.textContent = text;
  tip.style.display = "block";
  tip.style.left = "0px";
  tip.style.top = "0px";
  const r = owner.getBoundingClientRect();
  const t = tip.getBoundingClientRect();
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  let x = r.left + r.width / 2 - t.width / 2;
  x = Math.max(4, Math.min(x, vw - t.width - 4));
  // 下に出す。入らなければ上に
  let y = r.bottom + GAP;
  if (y + t.height > vh - 4) y = Math.max(4, r.top - GAP - t.height);
  tip.style.left = `${Math.round(x)}px`;
  tip.style.top = `${Math.round(y)}px`;
  shown = true;
}

function show(owner: Element) {
  const text = adopt(owner);
  if (!text) {
    hide();
    return;
  }
  place(owner, text);
}

function onMove(e: PointerEvent) {
  if (e.pointerType === "touch") return;
  const owner = tipOwner(e.target as Element | null);
  if (owner !== current) {
    clearTimeout(timer);
    if (shown) {
      lastHidden = Date.now();
      shown = false;
      if (tip) tip.style.display = "none";
    }
    release(current);
    current = owner;
    if (!owner) return;
    adopt(owner);
    const warm = Date.now() - lastHidden < WARM_MS;
    timer = setTimeout(() => current === owner && show(owner), warm ? 0 : DELAY_MS);
    return;
  }
  // 載っている間に title が書き換わった(ぶつかりの印・つまみの値など)→ 読み直す
  if (owner && owner.hasAttribute("title")) {
    const text = adopt(owner);
    if (shown) {
      if (text) place(owner, text);
      else hide();
    }
  }
}

/** 画面全体に吹き出しを付ける(起動時に 1 回) */
export function installTooltips(): () => void {
  tip = document.createElement("div");
  tip.className = "glaux-tip";
  tip.setAttribute("role", "tooltip");
  tip.style.display = "none";
  document.body.appendChild(tip);
  const onDown = () => hide();
  const onLeave = (e: PointerEvent) => {
    if (!e.relatedTarget) hide();
  };
  document.addEventListener("pointermove", onMove, true);
  document.addEventListener("pointerdown", onDown, true);
  document.addEventListener("pointerout", onLeave, true);
  document.addEventListener("wheel", onDown, { capture: true, passive: true });
  document.addEventListener("keydown", onDown, true);
  window.addEventListener("blur", onDown);
  return () => {
    document.removeEventListener("pointermove", onMove, true);
    document.removeEventListener("pointerdown", onDown, true);
    document.removeEventListener("pointerout", onLeave, true);
    document.removeEventListener("wheel", onDown, true);
    document.removeEventListener("keydown", onDown, true);
    window.removeEventListener("blur", onDown);
    tip?.remove();
    tip = null;
  };
}
