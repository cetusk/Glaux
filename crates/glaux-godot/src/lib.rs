//! # glaux-godot
//!
//! Glaux の曲(`.glaux` フォルダ)を Godot 4 のゲームの中で鳴らし、敵の動きなどを曲に同期させる拡張。
//!
//! - [`GlauxPlayer`](player::GlauxPlayer)(Node): 曲を読み込んで再生し、「いま聞こえている位置」で
//!   拍・マーカー・監視トラックのノートをシグナルで知らせる。先読みの問い合わせもできる
//! - 音は Glaux の再生エンジン(`glaux_engine::render::Renderer`)を Godot の音声スレッドから直接呼んで作る
//!   ([`GlauxStream`](stream::GlauxStream))。DAW で聴いた音がそのままゲームで鳴る
//! - ファイルは Godot の `FileAccess` で読むので、書き出したゲーム(.pck の中)でも読める
//!
//! 制限: CLAP プラグイン(Surge XT 等)の音源・エフェクトはゲームでは鳴らない(読み込み時に警告し、
//! その音源のトラックは無音にする)。

use godot::prelude::*;

mod player;
mod song;
mod stream;

struct GlauxExtension;

#[gdextension]
unsafe impl ExtensionLibrary for GlauxExtension {}
