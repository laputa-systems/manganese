pub const NOPTS: i64 = i64::MIN;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rational {
    pub num: i32,
    pub den: i32,
}

impl Rational {
    pub const fn new(num: i32, den: i32) -> Self {
        Rational { num, den }
    }
}

pub const MICROS: Rational = Rational::new(1, 1_000_000);
pub const TIME_BASE: Rational = Rational::new(1, 1_000_000);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Rounding {
    Down = 2,
    Up = 3,
    NearInf = 5,
}

pub fn rescale_rnd(a: i64, b: i64, c: i64, rnd: Rounding, pass_minmax: bool) -> i64 {
    if c <= 0 || b < 0 {
        return i64::MIN;
    }
    if pass_minmax && (a == i64::MIN || a == i64::MAX) {
        return a;
    }
    let mode = rnd as i64;
    if a < 0 {
        let flipped = match rnd {
            Rounding::Down => Rounding::Up,
            Rounding::Up => Rounding::Down,
            other => other,
        };
        return (rescale_rnd(-a.max(-i64::MAX), b, c, flipped, false) as u64).wrapping_neg() as i64;
    }
    let r: i64 = if mode == Rounding::NearInf as i64 {
        c / 2
    } else if mode & 1 != 0 {
        c - 1
    } else {
        0
    };
    if b <= i32::MAX as i64 && c <= i32::MAX as i64 {
        if a <= i32::MAX as i64 {
            return (a * b + r) / c;
        }
        let ad = a / c;
        let a2 = (a % c * b + r) / c;
        if ad >= i32::MAX as i64 && b != 0 && ad > (i64::MAX - a2) / b {
            return i64::MIN;
        }
        return ad * b + a2;
    }
    let quotient = (a as i128 * b as i128 + r as i128) / c as i128;
    let low = quotient as u64;
    if low > i64::MAX as u64 {
        return i64::MIN;
    }
    low as i64
}

pub fn rescale(a: i64, b: i64, c: i64) -> i64 {
    rescale_rnd(a, b, c, Rounding::NearInf, false)
}

pub fn rescale_q_rnd(a: i64, bq: Rational, cq: Rational, rnd: Rounding, pass_minmax: bool) -> i64 {
    let b = bq.num as i64 * cq.den as i64;
    let c = cq.num as i64 * bq.den as i64;
    rescale_rnd(a, b, c, rnd, pass_minmax)
}

pub fn rescale_q(a: i64, bq: Rational, cq: Rational) -> i64 {
    rescale_q_rnd(a, bq, cq, Rounding::NearInf, false)
}

fn gcd(a: i64, b: i64) -> i64 {
    let (mut a, mut b) = (a, b);
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

pub fn reduce(num: i64, den: i64, max: i64) -> (bool, Rational) {
    let mut a0 = (0i64, 1i64);
    let mut a1 = (1i64, 0i64);
    let sign = (num < 0) ^ (den < 0);
    let mut num = num.abs();
    let mut den = den.abs();
    let divisor = gcd(num, den);
    if divisor != 0 {
        num /= divisor;
        den /= divisor;
    }
    if num <= max && den <= max {
        a1 = (num, den);
        den = 0;
    }
    while den != 0 {
        let x = (num as u64 / den as u64) as i64;
        let next_den = num - den * x;
        let a2n = x * a1.0 + a0.0;
        let a2d = x * a1.1 + a0.1;
        if a2n > max || a2d > max {
            let mut x = x;
            if a1.0 != 0 {
                x = (max - a0.0) / a1.0;
            }
            if a1.1 != 0 {
                x = x.min((max - a0.1) / a1.1);
            }
            if den * (2 * x * a1.1 + a0.1) > num * a1.1 {
                a1 = (x * a1.0 + a0.0, x * a1.1 + a0.1);
            }
            break;
        }
        a0 = a1;
        a1 = (a2n, a2d);
        num = den;
        den = next_den;
    }
    let value = Rational::new(if sign { -a1.0 as i32 } else { a1.0 as i32 }, a1.1 as i32);
    (den == 0, value)
}

pub fn mul_q(b: Rational, c: Rational) -> Rational {
    reduce(b.num as i64 * c.num as i64, b.den as i64 * c.den as i64, i32::MAX as i64).1
}

pub fn add_stable(ts_tb: Rational, ts: i64, inc_tb: Rational, inc: i64) -> i64 {
    let inc_tb = if inc != 1 { mul_q(inc_tb, Rational::new(inc as i32, 1)) } else { inc_tb };
    let m = inc_tb.num as i64 * ts_tb.den as i64;
    let d = inc_tb.den as i64 * ts_tb.num as i64;
    if m % d == 0 && ts <= i64::MAX - m / d {
        return ts + m / d;
    }
    if m < d {
        return ts;
    }
    let old = rescale_q(ts, ts_tb, inc_tb);
    let old_ts = rescale_q(old, inc_tb, ts_tb);
    if old == i64::MAX || old == NOPTS || old_ts == NOPTS {
        return ts;
    }
    rescale_q(old + 1, inc_tb, ts_tb).saturating_add(ts - old_ts)
}

pub fn log2(value: u32) -> u32 {
    if value == 0 { 0 } else { 31 - value.leading_zeros() }
}
