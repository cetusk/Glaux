//! 撥弦(pluck)の弦のバッファの置き場。
//!
//! 弦のリングバッファは 1 本 8KB あり、ボイスの型に直に持たせると、どの音源のボイスもその大きさになって
//! ボイスの追加・入れ替えのたびに 8KB をコピーしていた。バッファは起動時に 1 回だけ確保したこの置き場から
//! 借り、ボイスが消えるときに返す。借りる・返すはビットの原子的な操作だけで、オーディオスレッドで
//! 確保もロックもしない。置き場が尽きたら借りられず、その音は鳴らない(上限は同時発音数より十分多い)。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// 弦 1 本の長さ(サンプル)。48kHz で約 23Hz まで
pub const STRING_LEN: usize = 2048;
/// 置き場の本数(再生・書き出し・試聴を合わせた同時発音数より十分多く)
const POOL_SIZE: usize = 1024;
const WORDS: usize = POOL_SIZE / 64;

struct Pool {
    used: [AtomicU64; WORDS],
    /// POOL_SIZE × STRING_LEN の f32(0 で確保。触るまで OS は実メモリを割り当てない)
    data: *mut f32,
}

// 各バッファは借りている 1 つの StringBuf だけが触る(used のビットで排他)
unsafe impl Send for Pool {}
unsafe impl Sync for Pool {}

fn pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| {
        let v = vec![0.0f32; POOL_SIZE * STRING_LEN].into_boxed_slice();
        Pool {
            used: std::array::from_fn(|_| AtomicU64::new(0)),
            data: Box::leak(v).as_mut_ptr(),
        }
    })
}

/// 置き場を用意する(オーディオスレッドより前に、エンジンの起動時に呼ぶ。最初の 1 回だけ確保する)
pub fn init() {
    let _ = pool();
}

/// 借りている弦のバッファ 1 本
#[derive(Debug)]
pub struct StringBuf {
    idx: u32,
}

impl StringBuf {
    /// 空いているバッファを借りる(中身は 0 にする)。空きが無ければ None
    pub fn alloc() -> Option<StringBuf> {
        let p = pool();
        for (w, word) in p.used.iter().enumerate() {
            let mut cur = word.load(Ordering::Relaxed);
            while cur != u64::MAX {
                let bit = (!cur).trailing_zeros();
                match word.compare_exchange_weak(
                    cur,
                    cur | (1u64 << bit),
                    Ordering::Acquire,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        let mut b = StringBuf {
                            idx: (w * 64) as u32 + bit,
                        };
                        b.as_mut_slice().fill(0.0);
                        return Some(b);
                    }
                    Err(now) => cur = now,
                }
            }
        }
        None
    }

    pub fn as_slice(&self) -> &[f32] {
        // SAFETY: idx のバッファはこの StringBuf だけが借りている(used のビットで排他)
        unsafe {
            std::slice::from_raw_parts(pool().data.add(self.idx as usize * STRING_LEN), STRING_LEN)
        }
    }

    pub fn as_mut_slice(&mut self) -> &mut [f32] {
        // SAFETY: 同上。&mut self なのでこの借り手の中でも 1 つだけ
        unsafe {
            std::slice::from_raw_parts_mut(
                pool().data.add(self.idx as usize * STRING_LEN),
                STRING_LEN,
            )
        }
    }
}

impl Drop for StringBuf {
    fn drop(&mut self) {
        let (w, bit) = (self.idx as usize / 64, self.idx % 64);
        pool().used[w].fetch_and(!(1u64 << bit), Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffers_are_exclusive_and_returned() {
        let mut a = StringBuf::alloc().unwrap();
        let b = StringBuf::alloc().unwrap();
        assert_ne!(a.idx, b.idx);
        a.as_mut_slice()[5] = 1.0;
        assert_eq!(b.as_slice()[5], 0.0);
        let ia = a.idx;
        drop(a);
        // 返したものは再び借りられ、中身は 0 に戻る
        // (ほかのテストも同時に借りるので、見つかったときだけ確かめる)
        let mut again = Vec::new();
        while let Some(c) = StringBuf::alloc() {
            if c.idx == ia {
                assert_eq!(c.as_slice()[5], 0.0);
                break;
            }
            again.push(c);
        }
    }
}

#[cfg(test)]
mod size_tests {
    #[test]
    fn voices_are_small() {
        // 弦のバッファを外に出したので、ボイスは 8KB を持たない(以前は 8.4KB)
        let n = std::mem::size_of::<crate::VoiceState>();
        eprintln!("VoiceState = {n} B");
        assert!(n < 2048, "{n}");
    }
}
