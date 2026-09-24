//! Godot の音声スレッドから Glaux のレンダラを呼ぶ `AudioStream`。
//!
//! Godot は `AudioStreamPlayback::_mix` を音声スレッドで呼ぶ。そこで [`Renderer::process`] を回して
//! 波形を作る(レンダラはアロケーション・ロックなしで動く RT セーフな作り)。再生・停止・シークは
//! 本体(メインスレッド)が [`Shared`] のアトミックを書き換えて伝える。

use glaux_engine::render::{Renderer, Shared};
use godot::classes::native::AudioFrame;
use godot::classes::{AudioStream, AudioStreamPlayback, IAudioStream, IAudioStreamPlayback};
use godot::prelude::*;
use godot_core::meta::RawPtr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// 1 回のレンダリングで作るフレーム数の上限(Godot がもっと多く求めたら分けて作る)
const CHUNK: usize = 1024;

/// 音声スレッドと本体の間の時計。
#[derive(Default)]
pub struct MixClock {
    /// 直前のミックスの頭の再生位置(サンプル)。「いま聞こえている位置」の推定に使う
    pub mix_start: AtomicU64,
    /// ミックスした回数(音声スレッドが動いているかの確認用)
    pub mixes: AtomicU64,
    /// 直前のミックスの頭のレンダラの時計(サンプル)。時刻指定のノートの位置合わせに使う
    pub mix_clock: AtomicU64,
}

/// Glaux の曲を鳴らすストリーム。[`GlauxPlayer`](crate::player::GlauxPlayer) が作って
/// 自分の `AudioStreamPlayer` に設定する(GDScript から直接作る必要はない)。
#[derive(GodotClass)]
#[class(init, base=AudioStream)]
pub struct GlauxStream {
    base: Base<AudioStream>,
    pub shared: Option<Arc<Shared>>,
    pub clock: Arc<MixClock>,
}

#[godot_api]
impl IAudioStream for GlauxStream {
    fn instantiate_playback(&self) -> Option<Gd<AudioStreamPlayback>> {
        let shared = self.shared.clone()?;
        let clock = self.clock.clone();
        let pb = Gd::from_init_fn(|base| GlauxPlayback {
            base,
            renderer: Renderer::new(shared.clone()),
            shared,
            clock,
            buf: vec![0.0; CHUNK * 2],
            active: false,
        });
        Some(pb.upcast())
    }

    fn get_stream_name(&self) -> GString {
        "Glaux".into()
    }

    fn get_length(&self) -> f64 {
        0.0
    }
}

#[derive(GodotClass)]
#[class(no_init, base=AudioStreamPlayback)]
pub struct GlauxPlayback {
    base: Base<AudioStreamPlayback>,
    renderer: Renderer,
    shared: Arc<Shared>,
    clock: Arc<MixClock>,
    /// レンダラの出力(ステレオ・インターリーブ)。起動時に確保して使い回す
    buf: Vec<f32>,
    active: bool,
}

#[godot_api]
impl IAudioStreamPlayback for GlauxPlayback {
    fn start(&mut self, _from_pos: f64) {
        self.active = true;
    }

    fn stop(&mut self) {
        self.active = false;
    }

    fn is_playing(&self) -> bool {
        self.active
    }

    unsafe fn mix_rawptr(
        &mut self,
        buffer: RawPtr<*mut AudioFrame>,
        _rate_scale: f32,
        frames: i32,
    ) -> i32 {
        let n = frames.max(0) as usize;
        // SAFETY: Godot は frames 個の AudioFrame を書ける領域を渡す
        let out = unsafe { std::slice::from_raw_parts_mut(buffer.ptr(), n) };
        self.clock
            .mix_start
            .store(self.shared.pos.load(Ordering::Acquire), Ordering::Release);
        self.clock
            .mix_clock
            .store(self.renderer.clock(), Ordering::Release);
        self.clock.mixes.fetch_add(1, Ordering::Relaxed);
        let mut done = 0;
        while done < n {
            let m = (n - done).min(CHUNK);
            let b = &mut self.buf[..m * 2];
            self.renderer.process(b, 2);
            for (f, s) in out[done..done + m].iter_mut().zip(b.as_chunks::<2>().0) {
                f.left = s[0];
                f.right = s[1];
            }
            done += m;
        }
        frames
    }
}
