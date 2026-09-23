# 同梱モデル

| ファイル | 出典 | ライセンス |
|---|---|---|
| `swiftf0.onnx` | [lars76/swift-f0](https://github.com/lars76/swift-f0) `swift_f0/model.onnx`(2026-09 時点の main)を onnxruntime の基本最適化(ORT_ENABLE_BASIC)で定数畳み込みしたもの | MIT(`LICENSE-swiftf0`、Copyright 2025-2026 Lars Nieradzik) |
| `basic_pitch_nmp.onnx` | [spotify/basic-pitch](https://github.com/spotify/basic-pitch) `basic_pitch/saved_models/icassp_2022/nmp.onnx` | Apache-2.0(`LICENSE-basic-pitch`、Copyright 2022 Spotify AB) |

basic-pitch: Bittner et al., "A Lightweight Instrument-Agnostic Model for Polyphonic Note
Transcription and Multipitch Estimation", ICASSP 2022.

SwiftF0: Nieradzik, "SwiftF0: Fast and Accurate Monophonic Pitch Detection", 2025
(https://arxiv.org/abs/2508.18440)。約 9.6 万パラメータ、16kHz 入力・16ms ごとに音程と確からしさを返す。
