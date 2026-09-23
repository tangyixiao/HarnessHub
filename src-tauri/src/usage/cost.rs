//! 金额边界：把外部 JSON 里的**十进制原文**转成整数微单位。
//!
//! ADR-0011 决策六：金额不用浮点表示。
//!
//! 输入刻意是 `&str`（来自 `serde_json::value::RawValue` 的原文），不是 `f64`：
//! ccusage 实测会输出 `0.03258976559999999` 这种带二进制噪声的数字，
//! 先变成 f64 再乘 1e6 会让舍入结果依赖浮点误差，既不可预测也不可测。
//!
//! 取整规则固定为 **round-to-nearest, ties-to-even**，全部用整数运算完成。

use crate::error::{Error, Result};

/// 一个货币单位的百万分之一。
pub const MICROUNITS_PER_UNIT: i64 = 1_000_000;

/// 十进制原文里允许的最大有效位数（防止 `1e300` 这类输入把整数运算撑爆）。
const MAX_DIGITS: usize = 30;

/// 把十进制原文转换成整数微单位。
///
/// ```text
/// "0.03258976559999999" -> 32590
/// "21.120615000000004"  -> 21120615
/// "1.5e-6"              -> 2        （恰好半个微单位，ties-to-even）
/// "2.5e-6"              -> 2
/// ```
pub fn decimal_to_microunits(text: &str) -> Result<i64> {
    let input = text.trim();
    let invalid = |reason: &str| Error::InvalidInput(format!("金额 {input:?} 无法解析：{reason}"));

    if input.is_empty() {
        return Err(invalid("空字符串"));
    }

    let (negative, rest) = match input.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, input.strip_prefix('+').unwrap_or(input)),
    };

    // 指数部分：JSON.stringify 对 <1e-6 的数字会输出 "5.5e-7" 这种形式。
    let (mantissa, exponent) = match rest.find(['e', 'E']) {
        Some(index) => {
            let (mantissa, tail) = rest.split_at(index);
            let digits = &tail[1..];
            if digits.is_empty() {
                return Err(invalid("指数为空"));
            }
            let (sign, digits) = match digits.strip_prefix('-') {
                Some(digits) => (-1i32, digits),
                None => (1i32, digits.strip_prefix('+').unwrap_or(digits)),
            };
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err(invalid("指数不是整数"));
            }
            let magnitude: i32 = digits.parse().map_err(|_| invalid("指数超出可表示范围"))?;
            (mantissa, sign * magnitude)
        }
        None => (rest, 0),
    };

    let (integer_part, fraction_part) = match mantissa.split_once('.') {
        Some((integer, fraction)) => (integer, fraction),
        None => (mantissa, ""),
    };
    if integer_part.is_empty() && fraction_part.is_empty() {
        return Err(invalid("没有数字"));
    }
    if !integer_part.bytes().all(|b| b.is_ascii_digit())
        || !fraction_part.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(invalid("出现非数字字符"));
    }

    let mut digits = String::with_capacity(integer_part.len() + fraction_part.len());
    digits.push_str(integer_part);
    digits.push_str(fraction_part);

    // 前导零不携带信息，而且会把「有效位数」算多（"0.0000005" 只有 1 位有效数字）。
    let stripped = digits.trim_start_matches('0');
    if stripped.is_empty() {
        return Ok(0);
    }
    if stripped.len() > MAX_DIGITS {
        return Err(invalid("有效位数过多"));
    }
    let significand: i128 = stripped.parse().map_err(|_| invalid("有效数字无法解析"))?;

    // value  = significand × 10^(exponent - fraction_len)
    // result = round(value × 10^6)
    let scale = exponent - fraction_part.len() as i32 + 6;

    let magnitude = if scale >= 0 {
        let factor = pow10(scale as u32).ok_or_else(|| invalid("数值过大"))?;
        significand
            .checked_mul(factor)
            .ok_or_else(|| invalid("数值过大"))?
    } else {
        let divisor = pow10((-scale) as u32).ok_or_else(|| invalid("数值过小"))?;
        let quotient = significand / divisor;
        let remainder = significand % divisor;
        // ties-to-even：正好一半时取偶，其余按最近取整。
        let doubled = remainder * 2;
        if doubled > divisor || (doubled == divisor && quotient % 2 == 1) {
            quotient + 1
        } else {
            quotient
        }
    };

    let signed = if negative { -magnitude } else { magnitude };
    i64::try_from(signed).map_err(|_| invalid("超出 i64 可表示范围"))
}

fn pow10(exponent: u32) -> Option<i128> {
    let mut value: i128 = 1;
    for _ in 0..exponent {
        value = value.checked_mul(10)?;
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_the_float_noise_that_ccusage_actually_emits() {
        // 实测原文（fixtures/ccusage/README.md）。先转 f64 再乘 1e6 会依赖浮点误差。
        assert_eq!(
            decimal_to_microunits("0.03258976559999999").expect("解析"),
            32_590
        );
        assert_eq!(
            decimal_to_microunits("21.120615000000004").expect("解析"),
            21_120_615
        );
        assert_eq!(
            decimal_to_microunits("288.8240692563998").expect("解析"),
            288_824_069
        );
        assert_eq!(
            decimal_to_microunits("17.9204936").expect("解析"),
            17_920_494
        );
    }

    #[test]
    fn rounds_half_to_even() {
        assert_eq!(decimal_to_microunits("0.0000005").expect("解析"), 0);
        assert_eq!(decimal_to_microunits("0.0000015").expect("解析"), 2);
        assert_eq!(decimal_to_microunits("0.0000025").expect("解析"), 2);
        assert_eq!(decimal_to_microunits("0.0000035").expect("解析"), 4);
    }

    #[test]
    fn rounds_away_from_exact_half_values() {
        assert_eq!(decimal_to_microunits("0.0000014").expect("解析"), 1);
        assert_eq!(decimal_to_microunits("0.0000016").expect("解析"), 2);
        assert_eq!(decimal_to_microunits("0.0000004").expect("解析"), 0);
        assert_eq!(decimal_to_microunits("0.0000006").expect("解析"), 1);
    }

    #[test]
    fn handles_exponent_notation() {
        // JSON.stringify 对 <1e-6 的数字会输出指数形式，这不是假设而是可复现的。
        assert_eq!(decimal_to_microunits("1e-6").expect("解析"), 1);
        assert_eq!(decimal_to_microunits("1.5e-6").expect("解析"), 2);
        assert_eq!(decimal_to_microunits("2.5e-6").expect("解析"), 2);
        assert_eq!(decimal_to_microunits("5.5e-7").expect("解析"), 1);
        assert_eq!(decimal_to_microunits("1e-7").expect("解析"), 0);
        assert_eq!(decimal_to_microunits("1.25E+2").expect("解析"), 125_000_000);
    }

    #[test]
    fn handles_sign_and_zero() {
        assert_eq!(decimal_to_microunits("0").expect("解析"), 0);
        assert_eq!(decimal_to_microunits("0.000").expect("解析"), 0);
        assert_eq!(decimal_to_microunits("-0.0").expect("解析"), 0);
        assert_eq!(decimal_to_microunits("3").expect("解析"), 3_000_000);
        assert_eq!(decimal_to_microunits("-1.5e-6").expect("解析"), -2);
    }

    #[test]
    fn rejects_garbage_instead_of_guessing() {
        // 宁可让导入带着清晰的错误失败，也不要把坏金额静默变成 0。
        for bad in [
            "", "   ", "abc", "1.2.3", "1e", "1e+", "1e1.5", "--1", "1,5", "0x10", "1_000", ".",
            "1e400",
        ] {
            assert!(
                decimal_to_microunits(bad).is_err(),
                "{bad:?} 必须被拒绝，不能猜一个值"
            );
        }
    }

    #[test]
    fn rejects_values_that_overflow_i64_microunits() {
        assert!(
            decimal_to_microunits("1e30").is_err(),
            "1e30 微单位放不进 i64"
        );
        assert!(decimal_to_microunits("999999999999999999999999").is_err());
    }
}
