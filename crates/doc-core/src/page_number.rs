//! 页码格式(`w:pgNumType@w:fmt`)与页码文字化:纯函数,无 IO。

/// 页码格式(`w:pgNumType@w:fmt`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PageNumFormat {
    /// 阿拉伯数字(`decimal`,缺省)。
    #[default]
    Decimal,
    /// 小写罗马数字(`lowerRoman`)。
    LowerRoman,
    /// 大写罗马数字(`upperRoman`)。
    UpperRoman,
    /// 小写字母(`lowerLetter`:a..z, aa..zz, aaa...)。
    LowerLetter,
    /// 大写字母(`upperLetter`)。
    UpperLetter,
    /// 其它 `w:fmt` 取值(`ordinal` / 各语种计数法 …):按 `Decimal` 输出,渲染侧发告警。
    Other,
}

impl PageNumFormat {
    /// 由 `w:fmt` 属性值解析;未识别的取值落 [`PageNumFormat::Other`](容错,不报错)。
    pub fn from_attr(value: &str) -> Self {
        match value {
            "decimal" => PageNumFormat::Decimal,
            "lowerRoman" => PageNumFormat::LowerRoman,
            "upperRoman" => PageNumFormat::UpperRoman,
            "lowerLetter" => PageNumFormat::LowerLetter,
            "upperLetter" => PageNumFormat::UpperLetter,
            _ => PageNumFormat::Other,
        }
    }
}

/// 把页码 `n` 按 `fmt` 文字化。罗马数字只覆盖 1..=3999、字母格式只覆盖 n >= 1,
/// 范围外降级为阿拉伯数字;`Other` 一律阿拉伯数字。
pub fn format_page_number(n: i64, fmt: PageNumFormat) -> String {
    let text = match fmt {
        PageNumFormat::LowerRoman => roman(n).map(|s| s.to_lowercase()),
        PageNumFormat::UpperRoman => roman(n),
        PageNumFormat::LowerLetter => letters(n),
        PageNumFormat::UpperLetter => letters(n).map(|s| s.to_uppercase()),
        PageNumFormat::Decimal | PageNumFormat::Other => None,
    };
    text.unwrap_or_else(|| n.to_string())
}

/// 大写罗马数字(减法记法),仅 1..=3999。
fn roman(n: i64) -> Option<String> {
    const TABLE: [(i64, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    if !(1..=3999).contains(&n) {
        return None;
    }
    let mut rest = n;
    let mut out = String::new();
    for (value, symbol) in TABLE {
        while rest >= value {
            out.push_str(symbol);
            rest -= value;
        }
    }
    Some(out)
}

/// 小写字母页码(Word 规则):1..=26 → a..z,27..=52 → aa..zz,53.. → aaa…(同一字母重复),n >= 1。
fn letters(n: i64) -> Option<String> {
    if n < 1 {
        return None;
    }
    let idx = (n - 1) % 26;
    let repeat = (n - 1) / 26 + 1;
    let letter = char::from(b'a' + idx as u8);
    Some(std::iter::repeat_n(letter, repeat as usize).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(n: i64, fmt: PageNumFormat) -> String {
        format_page_number(n, fmt)
    }

    #[test]
    fn decimal_and_other_print_arabic() {
        assert_eq!(f(7, PageNumFormat::Decimal), "7");
        assert_eq!(f(7, PageNumFormat::Other), "7");
        assert_eq!(f(0, PageNumFormat::Decimal), "0");
    }

    #[test]
    fn roman_numerals_follow_subtractive_notation() {
        for (n, lower) in [
            (1, "i"),
            (4, "iv"),
            (9, "ix"),
            (14, "xiv"),
            (40, "xl"),
            (90, "xc"),
            (400, "cd"),
            (1994, "mcmxciv"),
            (3999, "mmmcmxcix"),
        ] {
            assert_eq!(f(n, PageNumFormat::LowerRoman), lower);
            assert_eq!(f(n, PageNumFormat::UpperRoman), lower.to_uppercase());
        }
    }

    #[test]
    fn roman_out_of_range_degrades_to_arabic() {
        for fmt in [PageNumFormat::LowerRoman, PageNumFormat::UpperRoman] {
            assert_eq!(f(0, fmt), "0");
            assert_eq!(f(-3, fmt), "-3");
            assert_eq!(f(4000, fmt), "4000");
        }
    }

    #[test]
    fn letters_repeat_like_word() {
        for (n, lower) in [
            (1, "a"),
            (2, "b"),
            (26, "z"),
            (27, "aa"),
            (28, "bb"),
            (52, "zz"),
            (53, "aaa"),
        ] {
            assert_eq!(f(n, PageNumFormat::LowerLetter), lower);
            assert_eq!(f(n, PageNumFormat::UpperLetter), lower.to_uppercase());
        }
    }

    #[test]
    fn letters_out_of_range_degrade_to_arabic() {
        assert_eq!(f(0, PageNumFormat::LowerLetter), "0");
        assert_eq!(f(-1, PageNumFormat::UpperLetter), "-1");
    }

    #[test]
    fn from_attr_maps_known_values_and_flags_the_rest() {
        assert_eq!(PageNumFormat::from_attr("decimal"), PageNumFormat::Decimal);
        assert_eq!(
            PageNumFormat::from_attr("lowerRoman"),
            PageNumFormat::LowerRoman
        );
        assert_eq!(
            PageNumFormat::from_attr("upperRoman"),
            PageNumFormat::UpperRoman
        );
        assert_eq!(
            PageNumFormat::from_attr("lowerLetter"),
            PageNumFormat::LowerLetter
        );
        assert_eq!(
            PageNumFormat::from_attr("upperLetter"),
            PageNumFormat::UpperLetter
        );
        assert_eq!(PageNumFormat::from_attr("ordinal"), PageNumFormat::Other);
        assert_eq!(PageNumFormat::from_attr(""), PageNumFormat::Other);
    }
}
