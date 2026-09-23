"""音色語の辞書(LAION-CLAP 音楽版の言葉側の埋め込み)を作る。

    uv pip install torch --index-url https://download.pytorch.org/whl/cpu
    uv pip install "transformers==4.46.3"   # Python 3.12 で確認
    python scripts/clap_vocab.py > crates/glaux-ml/data/clap_vocab.json

アプリは音声側(ONNX)だけを動かし、言葉側はこの事前計算を使う。各語はいくつかの言い回しの
埋め込みの平均(正規化済み)を、int8(最大値で割って 127 倍)+ base64 で保存する。
どんな音にもそこそこ近い語(例 "clean")が上位に来ないよう、いろいろな合成音(REFERENCE)との
類似度の平均と標準偏差も保存し、アプリ側は「その語としては珍しく近い度合い」(z 値)で並べる。
"""

import base64
import json
import sys

import numpy as np
import torch
from transformers import ClapModel, ClapProcessor

# larger_clap_music(音楽のみ)は Hugging Face 版の言葉側が壊れている(どの文もほぼ同じ埋め込みになり、
# logit_scale も 0.03 程度)ため、音楽と音声で学習した版を使う
MODEL = "laion/larger_clap_music_and_speech"

ADJ = ["a {} sound", "this sound is {}", "{}"]
NOUN = ["the sound of {}", "a {} sound", "{}"]

# (カテゴリ, 言い回しの型, [(英語, 日本語)])
VOCAB = [
    ("instrument", NOUN, [
        ("a synth pad", "シンセパッド"),
        ("a synth lead", "シンセリード"),
        ("a synth bass", "シンセベース"),
        ("a sub bass", "サブベース"),
        ("an 808 bass", "808 ベース"),
        ("a synth pluck", "シンセのプラック"),
        ("an arpeggiated synth", "アルペジオのシンセ"),
        ("a supersaw synth", "スーパーソウ"),
        ("an acid bass TB-303", "アシッドベース(303)"),
        ("an FM synth bell", "FM のベル"),
        ("a bell", "ベル"),
        ("an electric piano", "エレピ"),
        ("an acoustic piano", "ピアノ"),
        ("an organ", "オルガン"),
        ("a string ensemble", "ストリングス"),
        ("a brass section", "ブラス"),
        ("a choir", "クワイア"),
        ("a flute", "フルート"),
        ("an acoustic guitar", "アコースティックギター"),
        ("a clean electric guitar", "クリーンのエレキギター"),
        ("a distorted electric guitar", "歪んだエレキギター"),
        ("a bass guitar", "ベースギター"),
        ("a marimba", "マリンバ"),
        ("a harp", "ハープ"),
        ("a kick drum", "キック"),
        ("a snare drum", "スネア"),
        ("a hi-hat", "ハイハット"),
        ("a cymbal", "シンバル"),
        ("a hand clap", "クラップ"),
        ("a tom drum", "タム"),
        ("percussion", "パーカッション"),
        ("a drum loop", "ドラムループ"),
        ("a noise riser", "ノイズのライザー"),
        ("a sound effect", "効果音"),
        ("chiptune 8-bit video game music", "チップチューン(8 ビット)"),
        ("a human voice", "人の声"),
        ("whistling", "口笛"),
    ]),
    ("tone", ADJ, [
        ("bright", "明るい"),
        ("dark", "暗い"),
        ("warm", "温かい"),
        ("cold", "冷たい"),
        ("muffled", "こもった"),
        ("crisp", "歯切れのいい"),
        ("airy", "空気感のある"),
        ("thin", "細い"),
        ("thick and fat", "太い"),
        ("hollow", "うつろな(中が空いた)"),
        ("nasal", "鼻にかかった"),
        ("harsh", "耳に痛い"),
        ("soft", "柔らかい"),
        ("mellow", "まろやかな"),
        ("piercing", "突き刺さる"),
        ("rich", "豊かな"),
    ]),
    ("texture", ADJ, [
        ("clean", "クリーン"),
        ("distorted", "歪んだ"),
        ("gritty", "ざらついた"),
        ("fuzzy", "ファズっぽい"),
        ("metallic", "金属的な"),
        ("glassy", "ガラスのような"),
        ("wooden", "木のような"),
        ("breathy", "息っぽい"),
        ("noisy", "ノイジー"),
        ("smooth", "なめらか"),
        ("buzzy", "ビリビリした"),
        ("digital", "デジタルな"),
        ("analog", "アナログな"),
        ("lo-fi", "ローファイ"),
        ("vintage", "ヴィンテージ"),
        ("detuned", "デチューンした"),
    ]),
    ("envelope", ADJ, [
        ("plucked", "はじいた"),
        ("percussive", "打撃的"),
        ("punchy", "パンチのある"),
        ("sustained", "伸びる(持続する)"),
        ("slowly swelling", "ゆっくり立ち上がる"),
        ("staccato", "スタッカート"),
        ("long decaying", "長く減衰する"),
        ("short and tight", "短く締まった"),
    ]),
    ("movement", ADJ, [
        ("with vibrato", "ビブラートのかかった"),
        ("with tremolo", "トレモロのかかった"),
        ("wobbling", "ワブル(うねる)"),
        ("pulsing", "脈打つ"),
        ("evolving", "変化していく"),
        ("with a filter sweep", "フィルタースイープ"),
        ("with chorus", "コーラスのかかった"),
        ("with a phaser", "フェイザーのかかった"),
        ("rising in pitch", "音程が上がる"),
        ("falling in pitch", "音程が下がる"),
        ("gated", "ゲートで刻まれた"),
        ("static", "変化の少ない"),
    ]),
    ("space", ADJ, [
        ("reverberant", "残響の多い"),
        ("dry", "ドライ"),
        ("spacious", "広がりのある"),
        ("with echo and delay", "エコー(ディレイ)のかかった"),
        ("close and intimate", "近い"),
        ("distant", "遠い"),
    ]),
    ("mood", ADJ, [
        ("dreamy", "夢のような"),
        ("ethereal", "幻想的"),
        ("aggressive", "攻撃的"),
        ("dark and cinematic", "ダークでシネマティック"),
        ("happy", "楽しい"),
        ("sad", "悲しい"),
        ("calm", "落ち着いた"),
        ("energetic", "エネルギッシュ"),
        ("retro 80s", "80 年代風"),
        ("futuristic", "未来的"),
        ("cute", "かわいい"),
        ("eerie", "不気味"),
        ("epic", "壮大"),
    ]),
]


SR = 48000


def reference_sounds():
    """語ごとの「どんな音にも近い度合い」を測るための、いろいろな合成音(2 秒)。"""
    rng = np.random.default_rng(0)
    t = np.arange(SR * 2) / SR
    out = []

    def env(a, d):
        return np.minimum(1, t / max(a, 1e-3)) * np.exp(-t * d)

    def saw(f):
        return 2 * ((t * f) % 1) - 1

    def lowpass(x, fc):
        a = np.exp(-2 * np.pi * fc / SR)
        y = np.zeros_like(x)
        acc = 0.0
        for i, v in enumerate(x):
            acc = (1 - a) * v + a * acc
            y[i] = acc
        return y

    for f in [55, 110, 220, 440, 880]:
        out.append(np.sin(2 * np.pi * f * t) * env(0.01, 0.5))
        out.append(saw(f) * env(0.005, 1.0))
        out.append(np.sign(np.sin(2 * np.pi * f * t)) * env(0.3, 0.2))
        out.append(lowpass(saw(f), 800) * env(0.001, 6.0))
        out.append(np.tanh(5 * saw(f)) * env(0.01, 0.3))
    for d in [3, 10, 40]:
        n = rng.standard_normal(len(t))
        out.append(n * env(0.001, d))
        out.append(lowpass(n, 500) * env(0.001, d))
    out.append(np.sin(2 * np.pi * (50 * t + 100 * (1 - np.exp(-t * 30)) / 30)) * env(0.001, 8))
    for r in [[1, 2.76, 5.4], [1, 3.9, 9.2]]:
        out.append(sum(np.sin(2 * np.pi * 330 * k * t) / (i + 1) for i, k in enumerate(r)) * env(0.001, 2))
    chord = sum(saw(f) for f in [220, 277.2, 329.6]) / 3
    out.append(chord * env(0.5, 0.1))
    out.append(saw(110) * (0.5 + 0.5 * np.sin(2 * np.pi * 6 * t)))
    return [(x / (np.abs(x).max() + 1e-9) * 0.5).astype(np.float32) for x in out]


def main():
    model = ClapModel.from_pretrained(MODEL).eval()
    proc = ClapProcessor.from_pretrained(MODEL)
    ref = proc(audios=reference_sounds(), sampling_rate=SR, return_tensors="pt")
    with torch.no_grad():
        audio = model.get_audio_features(input_features=ref["input_features"])
    audio = torch.nn.functional.normalize(audio, dim=-1).numpy()
    entries = []
    for category, templates, words in VOCAB:
        for en, ja in words:
            texts = [t.format(en) for t in templates]
            inputs = proc(text=texts, return_tensors="pt", padding=True)
            with torch.no_grad():
                e = model.get_text_features(**inputs)
                if not torch.is_tensor(e):
                    e = e.pooler_output
            e = torch.nn.functional.normalize(e, dim=-1).mean(0)
            e = torch.nn.functional.normalize(e, dim=0).numpy()
            sims = audio @ e
            scale = float(np.abs(e).max())
            q = np.round(e / scale * 127).astype(np.int8)
            entries.append({
                "category": category,
                "en": en,
                "ja": ja,
                "ref_mean": round(float(sims.mean()), 5),
                "ref_std": round(float(sims.std()), 5),
                "scale": round(scale / 127, 8),
                "q": base64.b64encode(q.tobytes()).decode(),
            })
    json.dump({
        "model": MODEL,
        "dim": 512,
        "note": "言葉側の埋め込み(言い回しの平均、正規化済み)。値 = q(int8) × scale。"
                "ref_mean / ref_std = いろいろな合成音とのコサイン類似度の平均と標準偏差",
        "reference_sounds": len(audio),
        "entries": entries,
    }, sys.stdout, ensure_ascii=False, indent=1)


if __name__ == "__main__":
    main()
