// 右クリックメニューなどの浮かぶパネルを、画面の中に収める action。
// クリック位置にそのまま置くと、画面の右端・下端でメニューの一部が画面外に出ていた。

const MARGIN = 8;

/** 開いたときに、はみ出す分だけ左・上へずらす(`use:keepInView`) */
export function keepInView(node: HTMLElement) {
  const fit = () => {
    const r = node.getBoundingClientRect();
    if (r.right > window.innerWidth - MARGIN) {
      node.style.left = `${Math.max(MARGIN, window.innerWidth - r.width - MARGIN)}px`;
    }
    if (r.bottom > window.innerHeight - MARGIN) {
      node.style.top = `${Math.max(MARGIN, window.innerHeight - r.height - MARGIN)}px`;
    }
  };
  fit();
  // 中身(プリセット一覧など)が後から増えたときも合わせ直す
  const ro = new ResizeObserver(fit);
  ro.observe(node);
  return { destroy: () => ro.disconnect() };
}

/** 出てきた入力欄に入力位置を置いて中身を選ぶ(`use:focusNow`)。
 *  autofocus は、ダブルクリックやメニューの直後だと前に押したボタンに負けることがあった */
export function focusNow(node: HTMLInputElement | HTMLTextAreaElement) {
  requestAnimationFrame(() => {
    node.focus();
    node.select();
  });
}
