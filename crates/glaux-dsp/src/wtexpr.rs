//! ウェーブテーブルを数式で作るときの式(`wtedit::expr_frames`)。
//! 数・`x`(1 周期の中の位置 0〜1)・`t`(テーブルの位置 0〜1)・`pi`・`+ - * / ^`・括弧・関数を読む。
//! 関数: sin cos tan abs sqrt exp log tanh floor sign saw tri sqr(1 引数)、min max pow(2 引数)、clamp mix(3 引数)。
//! テーブルを作るとき(オーディオスレッドの外)にだけ使う。

#[derive(Debug, Clone)]
pub enum Expr {
    Num(f32),
    X,
    T,
    Neg(Box<Expr>),
    Bin(char, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

impl Expr {
    pub fn eval(&self, x: f32, t: f32) -> f32 {
        match self {
            Expr::Num(v) => *v,
            Expr::X => x,
            Expr::T => t,
            Expr::Neg(e) => -e.eval(x, t),
            Expr::Bin(op, a, b) => {
                let (a, b) = (a.eval(x, t), b.eval(x, t));
                match op {
                    '+' => a + b,
                    '-' => a - b,
                    '*' => a * b,
                    '/' => a / b,
                    _ => a.powf(b),
                }
            }
            Expr::Call(f, args) => {
                let v: Vec<f32> = args.iter().map(|e| e.eval(x, t)).collect();
                match (f.as_str(), v.as_slice()) {
                    ("sin", [a]) => a.sin(),
                    ("cos", [a]) => a.cos(),
                    ("tan", [a]) => a.tan(),
                    ("abs", [a]) => a.abs(),
                    ("sqrt", [a]) => a.sqrt(),
                    ("exp", [a]) => a.exp(),
                    ("log", [a]) => a.ln(),
                    ("tanh", [a]) => a.tanh(),
                    ("floor", [a]) => a.floor(),
                    ("sign", [a]) => a.signum(),
                    // 1 周期を 0〜1 とした基本の形(−1〜1)
                    ("saw", [a]) => 2.0 * a.rem_euclid(1.0) - 1.0,
                    ("tri", [a]) => 1.0 - 4.0 * (a.rem_euclid(1.0) - 0.5).abs(),
                    ("sqr", [a]) => {
                        if a.rem_euclid(1.0) < 0.5 {
                            1.0
                        } else {
                            -1.0
                        }
                    }
                    ("min", [a, b]) => a.min(*b),
                    ("max", [a, b]) => a.max(*b),
                    ("pow", [a, b]) => a.powf(*b),
                    ("clamp", [v, a, b]) => v.clamp(a.min(*b), a.max(*b)),
                    ("mix", [a, b, w]) => a + (b - a) * w,
                    _ => f32::NAN,
                }
            }
        }
    }
}

const FUNCS: &[(&str, usize)] = &[
    ("sin", 1),
    ("cos", 1),
    ("tan", 1),
    ("abs", 1),
    ("sqrt", 1),
    ("exp", 1),
    ("log", 1),
    ("tanh", 1),
    ("floor", 1),
    ("sign", 1),
    ("saw", 1),
    ("tri", 1),
    ("sqr", 1),
    ("min", 2),
    ("max", 2),
    ("pow", 2),
    ("clamp", 3),
    ("mix", 3),
];

struct P<'a> {
    s: &'a [u8],
    i: usize,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn peek(&mut self) -> Option<u8> {
        self.ws();
        self.s.get(self.i).copied()
    }
    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn err(&self, what: &str) -> String {
        format!("式の {} 文字目: {what}", self.i + 1)
    }
    // 足し算・引き算
    fn sum(&mut self, depth: u32) -> Result<Expr, String> {
        let mut a = self.prod(depth)?;
        loop {
            let op = match self.peek() {
                Some(b'+') => '+',
                Some(b'-') => '-',
                _ => return Ok(a),
            };
            self.i += 1;
            let b = self.prod(depth)?;
            a = Expr::Bin(op, Box::new(a), Box::new(b));
        }
    }
    // 掛け算・割り算
    fn prod(&mut self, depth: u32) -> Result<Expr, String> {
        let mut a = self.unary(depth)?;
        loop {
            let op = match self.peek() {
                Some(b'*') => '*',
                Some(b'/') => '/',
                _ => return Ok(a),
            };
            self.i += 1;
            let b = self.unary(depth)?;
            a = Expr::Bin(op, Box::new(a), Box::new(b));
        }
    }
    // 符号(−x^2 は −(x^2))
    fn unary(&mut self, depth: u32) -> Result<Expr, String> {
        if self.eat(b'-') {
            return Ok(Expr::Neg(Box::new(self.unary(depth)?)));
        }
        if self.eat(b'+') {
            return self.unary(depth);
        }
        self.power(depth)
    }
    // 累乗(右結合)
    fn power(&mut self, depth: u32) -> Result<Expr, String> {
        let a = self.atom(depth)?;
        if self.eat(b'^') {
            let b = self.unary(depth)?;
            return Ok(Expr::Bin('^', Box::new(a), Box::new(b)));
        }
        Ok(a)
    }
    fn atom(&mut self, depth: u32) -> Result<Expr, String> {
        if depth > 64 {
            return Err(self.err("入れ子が深すぎます"));
        }
        match self.peek() {
            Some(b'(') => {
                self.i += 1;
                let e = self.sum(depth + 1)?;
                if !self.eat(b')') {
                    return Err(self.err(") がありません"));
                }
                Ok(e)
            }
            Some(c) if c.is_ascii_digit() || c == b'.' => {
                let st = self.i;
                while self.i < self.s.len()
                    && (self.s[self.i].is_ascii_digit() || self.s[self.i] == b'.')
                {
                    self.i += 1;
                }
                // 指数表記(1e-3)
                if self.i < self.s.len() && (self.s[self.i] == b'e' || self.s[self.i] == b'E') {
                    let save = self.i;
                    self.i += 1;
                    if self.i < self.s.len() && (self.s[self.i] == b'-' || self.s[self.i] == b'+') {
                        self.i += 1;
                    }
                    if self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
                        while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
                            self.i += 1;
                        }
                    } else {
                        self.i = save;
                    }
                }
                let txt = std::str::from_utf8(&self.s[st..self.i]).unwrap_or("");
                txt.parse::<f32>()
                    .map(Expr::Num)
                    .map_err(|_| self.err(&format!("数が読めません: {txt}")))
            }
            Some(c) if c.is_ascii_alphabetic() => {
                let st = self.i;
                while self.i < self.s.len()
                    && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_')
                {
                    self.i += 1;
                }
                let name = std::str::from_utf8(&self.s[st..self.i])
                    .unwrap_or("")
                    .to_ascii_lowercase();
                match name.as_str() {
                    "x" => return Ok(Expr::X),
                    "t" => return Ok(Expr::T),
                    "pi" => return Ok(Expr::Num(std::f32::consts::PI)),
                    "tau" => return Ok(Expr::Num(std::f32::consts::TAU)),
                    _ => {}
                }
                let Some(&(_, n)) = FUNCS.iter().find(|(f, _)| *f == name) else {
                    return Err(self.err(&format!(
                        "知らない名前: {name}(x・t・pi・関数 {})",
                        FUNCS.iter().map(|f| f.0).collect::<Vec<_>>().join(" ")
                    )));
                };
                if !self.eat(b'(') {
                    return Err(self.err(&format!("{name} の後に ( を")));
                }
                let mut args = vec![self.sum(depth + 1)?];
                while self.eat(b',') {
                    args.push(self.sum(depth + 1)?);
                }
                if !self.eat(b')') {
                    return Err(self.err(") がありません"));
                }
                if args.len() != n {
                    return Err(self.err(&format!("{name} の引数は {n} つ(got {})", args.len())));
                }
                Ok(Expr::Call(name, args))
            }
            Some(c) => Err(self.err(&format!("読めない文字: {}", c as char))),
            None => Err(self.err("式が途中で終わっています")),
        }
    }
}

/// 式を読む(長さは 2,000 字まで)
pub fn parse(src: &str) -> Result<Expr, String> {
    if src.len() > 2000 {
        return Err("式が長すぎます(2,000 字まで)".to_owned());
    }
    let mut p = P {
        s: src.as_bytes(),
        i: 0,
    };
    let e = p.sum(0)?;
    if p.peek().is_some() {
        return Err(p.err("余分な文字があります"));
    }
    Ok(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_evaluates() {
        let e = parse("sin(2*pi*x) + t*0.5*sqr(3*x)").unwrap();
        assert!((e.eval(0.25, 0.0) - 1.0).abs() < 1e-5);
        assert!(
            (e.eval(0.25, 1.0) - 0.5).abs() < 1e-5,
            "{}",
            e.eval(0.25, 1.0)
        );
        assert_eq!(parse("-x^2").unwrap().eval(3.0, 0.0), -9.0);
        assert_eq!(parse("2^3^2").unwrap().eval(0.0, 0.0), 512.0);
        assert_eq!(parse("clamp(x, 0, 0.5)").unwrap().eval(0.9, 0.0), 0.5);
        assert!((parse("1e-1 * 10").unwrap().eval(0.0, 0.0) - 1.0).abs() < 1e-6);
        for bad in ["sin(", "foo(x)", "x +", "min(1)", "(x", "x $"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }
}
