//! 道具の数の引数の範囲を、道具の入口でそろえて確かめる(#68)。
//!
//! 道具の中の足し算・掛け算は、とても大きな値(移調 2147483647 半音・40 億小節など)を想定していない。
//! debug では桁あふれの panic(受け止めて内部エラー)、release では黙って値が回り込んで壊れた音になる。
//! 名前ごとの範囲を 1 か所に集め、範囲の外なら道具を動かす前に、直し方の分かるエラーで返す。
//! 範囲は音楽として意味のある大きさより十分広く取る(ふつうの使い方を断らない)。

use glaux_core::time::{MAX_TICK, PPQ};
use serde_json::{Map, Value};

/// 小節の数の上限(曲の長さの上限 MAX_TICK は 4/4 で 10 万小節)
const MAX_BAR: i64 = 100_000;
/// 一度に書く・変える小節や回数の上限
const MAX_COUNT: i64 = 4096;

/// (道具の名前 または "*", 引数の名前, 下限, 上限)。道具の名前が合うものを先に見る
const RANGES: &[(&str, &str, i64, i64)] = &[
    // 道具ごとの例外(同じ名前でも意味が違う)
    ("transform_notes", "steps", -127, 127),
    ("transform_notes", "times", 0, 256),
    ("revise_melody", "steps", 0, 1000),
    // 小節の位置
    ("*", "bar", 0, MAX_BAR),
    ("*", "start_bar", 0, MAX_BAR),
    ("*", "land_bar", 0, MAX_BAR),
    ("*", "peak_bar", 0, MAX_BAR),
    // 小節の数・回数
    ("*", "bars", 0, MAX_COUNT),
    ("*", "count", 0, MAX_COUNT),
    ("*", "repeat", 0, MAX_COUNT),
    ("*", "roll_bars", 0, MAX_COUNT),
    ("*", "build_bars", 0, MAX_COUNT),
    ("*", "fill_every", 0, MAX_COUNT),
    ("*", "reset_every_bars", 0, MAX_COUNT),
    ("*", "motif_bars", 0, MAX_COUNT),
    ("*", "gap_beats", 0, MAX_COUNT),
    ("*", "gap_steps", 0, MAX_COUNT),
    ("*", "tries", 0, 1000),
    ("*", "candidates", 0, 1000),
    ("*", "max_evals", 0, 1000),
    // 音程(半音・度数)と拍
    ("*", "semitones", -127, 127),
    ("*", "interval", -127, 127),
    ("*", "shift", -127, 127),
    ("*", "steps", -127, 127),
    ("*", "beats", -64, 64),
    // 位置・ずらし量(tick)
    ("*", "to_tick", 0, MAX_TICK.0 as i64),
    ("*", "start_tick", 0, MAX_TICK.0 as i64),
    ("*", "delta_ticks", -(MAX_TICK.0 as i64), MAX_TICK.0 as i64),
    ("*", "offset_ticks", -(MAX_TICK.0 as i64), MAX_TICK.0 as i64),
    ("*", "grid_ticks", 0, (PPQ * 4 * 64) as i64),
    ("*", "quantize_ticks", 0, (PPQ * 4 * 64) as i64),
    // 音の大きさ・長さ
    ("*", "frames", 0, 4096),
    ("*", "rows", 0, 4096),
    ("*", "slices", 0, 64),
    ("*", "duration_ms", 0, 600_000),
];

fn range(tool: &str, arg: &str) -> Option<(i64, i64)> {
    RANGES
        .iter()
        .find(|(t, a, ..)| *t == tool && *a == arg)
        .or_else(|| RANGES.iter().find(|(t, a, ..)| *t == "*" && *a == arg))
        .map(|&(_, _, lo, hi)| (lo, hi))
}

/// 引数の数が範囲の中にあるか確かめる。外なら、どの引数をどの範囲にすればよいかを返す
pub fn check(tool: &str, args: &Map<String, Value>) -> Result<(), String> {
    for (k, v) in args {
        let Some((lo, hi)) = range(tool, k) else {
            continue;
        };
        // 整数として読めない(小数・文字)ものは、道具の側の型の確かめに任せる
        let n = match v {
            Value::Number(n) => n
                .as_i64()
                .map(i128::from)
                .or_else(|| n.as_u64().map(i128::from)),
            _ => None,
        };
        if let Some(n) = n {
            if n < lo as i128 || n > hi as i128 {
                return Err(glaux_core::tr!(
                    "{tool} の {k} = {n} は範囲の外です({lo}〜{hi})。値を見直してください",
                    "{tool}: {k} = {n} is out of range ({lo} to {hi}). Please check the value"
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn rejects_huge_values_and_keeps_normal_ones() {
        assert!(check("transpose_notes", &args(json!({"semitones": 12}))).is_ok());
        assert!(check("transpose_notes", &args(json!({"semitones": -24}))).is_ok());
        let e = check("transpose_notes", &args(json!({"semitones": 2147483647}))).unwrap_err();
        assert!(e.contains("semitones"), "{e}");
        assert!(check("plan_melody", &args(json!({"bars": 4294967295u64}))).is_err());
        assert!(check("plan_melody", &args(json!({"bars": 16}))).is_ok());
        assert!(check("change_meter", &args(json!({"beats": 2147483647}))).is_err());
        assert!(check("change_meter", &args(json!({"beats": -2}))).is_ok());
        assert!(check("shift_notes", &args(json!({"delta_ticks": i64::MAX}))).is_err());
        assert!(check("shift_notes", &args(json!({"delta_ticks": -3840}))).is_ok());
        // 同じ名前でも道具ごとの範囲を先に見る
        assert!(check("revise_melody", &args(json!({"steps": 500}))).is_ok());
        assert!(check("transform_notes", &args(json!({"steps": 500}))).is_err());
        // 知らない名前・数でない値は通す(道具の側で確かめる)
        assert!(check("x", &args(json!({"seed": u64::MAX, "bar": "3"}))).is_ok());
    }

    #[test]
    fn every_range_is_ordered() {
        for (t, a, lo, hi) in RANGES {
            assert!(lo <= hi, "{t}.{a}");
        }
    }
}
