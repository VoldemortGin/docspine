//! 公式 `m:oMath` / `m:oMathPara` → 线性记法纯文本(给 RAG 用:不排版,但不能读错)。
//!
//! 目标是**单射性的工程近似**:结构不同、数学含义不同的两个公式不输出相同文本,且输出按常规
//! 数学记法的优先级去读就是原式。规则(实现与 `math_props` 性质测试共同维护):
//!
//! 1. **原子**只认:单个字母(含非 ASCII 字母,可带组合重音符 U+0300 等)、纯 ASCII 数字串(至多一个
//!    小数点)、或被一对**相互匹配**的括号(`()` `[]` `{}` `⟨⟩` `⌊⌋` `⌈⌉`)完整包住的串;空串视作原子
//!    (空槽位不加括号)。`2x`、`ab`、`-1`、`αβ`、`x_i` 都不是原子。
//! 2. **槽位**(分子、分母、底、上下标、根号次数、极限、被重音修饰式)线性化后不是原子就加圆括号:
//!    `(a+b)/c`、`x^(n+1)`、`(2x)^2`、`(ab)^2`、`x^(-1)`、`(ab)̇`;根号写成 `sqrt(x)` / `root(n,x)`,
//!    被开方式已在括号里,不再重复加括号(次数非原子仍加)。`m:limLow` / `m:limUpp` 的底若是运算符名
//!    (纯 ASCII 字母词:`lim`、`max`、`sup`)不加括号:`lim_(x→0)`。
//!    **被作用式**(`∑` / `∫` 的被加 / 被积式、函数自变量)不是原子、也不是「带上下标的原子」(`x_i`、
//!    `x^2`、`x_i^2` 这类读者公认是一个整体的写法)时加括号:`∑_(i=1)^n x_i`、`∑_(i=1)^n (x_i+1)`、
//!    `sin x`、`sin(x+1)`、`sin(1/2)`。
//! 3. **相邻项之间的分隔**:一个槽位 / 公式是项的序列(文字 run 与结构)。文字与文字原样拼接
//!    (原文里本就是连续文字);只要有一侧是**结构**(分式、上下标、根号、求和、函数、前置上下标、
//!    重音、上下划线、极限),拼接处就插入一个空格,除非拼接处一侧是空白、运算符 / 关系符 / 标点
//!    (`+ - = < > , ; :` 等与 `^ _ / · × ÷ *`),或左侧以开括号结尾 / 右侧以闭括号开头。
//!    定界符 `m:d` 与矩阵两端是括号时按文字处理(`2(x+1)`、`(a)(b)`),两端不是括号时按结构处理。
//!    `m:oMathPara` 里相邻的 `m:oMath`、`m:eqArr` 的各行之间是**硬边界**:输出一个空格,两侧互不算相邻项。
//! 4. **乘法语境**:分式左右两侧任一侧(跳过空白)紧邻的不是低优先级运算符 / 关系符 / 标点 / 对应侧的
//!    括号时,分式整体加括号:`3 (1/2)`、`(1/2) x`、`2·(1/2)`;独立或与 `+`、`=` 相邻时仍是 `1/2`。
//!    函数自变量没加括号(`sin x`)而右侧紧邻乘法项时改写成 `sin(x) y`;带被作用式的 `∑` / `∫` 右侧
//!    紧邻乘法项时整体加括号:`(∑_(i=1)^n x_i) y`。
//! 5. 上下标的底不是原子就加括号:`(2x)^2` 与 `2 x^2`(2·x²)不同;`x^2 3`、`x_1 2`、`e^x y`、
//!    `x^2 y^2`、`sin x cos x`。结构与 `^` / `_` / `/`(左侧)或 `^` / `_` / `/` / `!` / 撇号(右侧)
//!    直接相邻且自身不是原子时整体加括号(`(x^2)!`)。
//! 6. `m:d`:`sepChr` 缺省 `|`;显式空串时用一个空格分隔多个 `m:e`,免得粘成一项。`begChr` / `endChr`
//!    缺省 `(` `)`,显式空串表示该侧无括号。
//! 7. `m:sPre` 写成 `(_下^上)底`:前置上下标整体包在括号里紧贴底数(底不是原子时加括号),与前项之间
//!    按规则 3 加空格:`x (_6^14)C`。选这个写法是因为 `_6^14 C` 里的空格会让上下标看起来属于前项。
//! 8. 其余多槽结构(`m:box` / `m:borderBox` / `m:phant` / `m:groupChr`)是透明外壳:子项直接并入外层
//!    序列;`m:eqArr` 各行之间是硬边界。不认识的元素同样透明,文字按文档顺序拼接。
//!
//! 实现:**迭代**遍历(显式栈,不递归),结构帧深度受 `MAX_NEST_DEPTH` 约束(更深的结构退化为纯拼接),
//! 深嵌套不会栈溢出;每层只在收拢时拼接一次文字,总开销与输入成线性(× 至多 `MAX_NEST_DEPTH` 层)。

use doc_core::model::TextRun;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use super::document::{read_text, MAX_NEST_DEPTH};
use super::{attr_of, local_name, skip_element};

/// 公式里需要线性化的结构(其余元素是透明外壳)。
#[derive(Clone, Copy, PartialEq)]
enum MathStruct {
    /// `m:f` 分式(`m:num` / `m:den`)-> `分子/分母`。
    Frac,
    /// `m:sSup` 上标(`m:e` / `m:sup`)-> `底^上`。
    Sup,
    /// `m:sSub` 下标(`m:e` / `m:sub`)-> `底_下`。
    Sub,
    /// `m:sSubSup`(`m:e` / `m:sub` / `m:sup`)-> `底_下^上`。
    SubSup,
    /// `m:sPre` 前置上下标(`m:sub` / `m:sup` / `m:e`)-> `(_下^上)底`。
    Pre,
    /// `m:rad` 根号(`m:deg` / `m:e`)-> `sqrt(x)`;带次数时 `root(次,x)`。
    Rad,
    /// `m:d` 定界符(多个 `m:e`;`m:dPr` 的 `begChr` / `endChr` / `sepChr`)-> `(a|b)`。
    Delim,
    /// `m:nary` 大运算符(`m:sub` / `m:sup` / `m:e`;`m:naryPr > m:chr`)-> `∑_(下)^(上) 被作用式`。
    Nary,
    /// `m:func` 函数(`m:fName` / `m:e`)-> `sin x` / `sin(x+1)`。
    Func,
    /// `m:limLow`(`m:e` / `m:lim`)-> `底_(极限)`;`m:limUpp` -> `底^(极限)`。
    LimLow,
    LimUpp,
    /// `m:acc` 重音符(`m:e`;`m:accPr > m:chr`,缺省 U+0302)-> `底` + 重音符。
    Acc,
    /// `m:bar` 上 / 下划线(`m:e`;`m:barPr > m:pos`)-> `overline(x)` / `underline(x)`。
    Bar,
    /// `m:m` 矩阵(多个 `m:mr`)-> `[a,b;c,d]`。
    Matrix,
    /// `m:mr` 矩阵的一行(多个 `m:e`)-> `a,b`;本身又是 `m:m` 的一个槽位。
    MatRow,
    /// `m:box` / `m:borderBox` / `m:phant` / `m:groupChr`:透明外壳,子项并入外层序列。
    Wrapper,
    /// `m:eqArr` 方程组:各行之间是硬边界(一个空格)。
    EqArr,
}

/// 结构内的槽位元素。
#[derive(Clone, Copy, PartialEq)]
enum MathSlot {
    Num,
    Den,
    Base,
    Sup,
    Sub,
    Deg,
    Lim,
    FName,
    Row,
    Cell,
}

/// 结构属性(`m:*Pr` 里的 `m:val`):定界符 / 运算符 / 重音符字符、`m:bar` 位置。
/// `None` = 未出现(取缺省);`Some("")` = 显式空串。
#[derive(Default)]
struct MathProps {
    beg: Option<String>,
    end: Option<String>,
    sep: Option<String>,
    chr: Option<String>,
    pos: Option<String>,
}

/// 项序列里的一项。
enum Item {
    /// 文字(`m:t` 原文,或两端是括号的定界符 / 矩阵):与相邻文字直接拼接。
    Text(String),
    /// 结构的线性化结果。
    Struct(StructOut),
    /// 硬边界(相邻 `m:oMath` / `m:eqArr` 的行之间):输出一个空格,两侧互不算相邻项。
    Break,
}

/// 一个结构的线性化结果,及其在乘法语境下的替代写法(规则 4)。
struct StructOut {
    text: String,
    /// 左 / 右侧(跳过空白)紧邻乘法项时改用的写法;`None` = 不随语境变化。
    alt: Option<String>,
    group_left: bool,
    group_right: bool,
}

impl StructOut {
    fn plain(text: String) -> Self {
        StructOut {
            text,
            alt: None,
            group_left: false,
            group_right: false,
        }
    }
}

/// 公式遍历栈上的一帧:容器 / 结构 / 槽位各自攒一份项序列。其余嵌套元素不开帧,只在帧内
/// 记 `other_depth`(外壳不挡住其内的结构)。
struct MathFrame {
    /// 本帧是哪种结构(`None` = 容器或槽位)。
    kind: Option<MathStruct>,
    /// 本帧若是槽位,它是哪个槽。
    slot: Option<MathSlot>,
    /// 容器 / 槽位帧的项序列;结构帧里是槽位之外的零散文字(收拢时接在结构之后,不丢字)。
    items: Vec<Item>,
    /// 结构帧:已收齐的槽位 `(槽, 项序列)`(同名槽可重复,如 `m:d` 的多个 `m:e`)。
    slots: Vec<(MathSlot, Vec<Item>)>,
    /// 结构帧:`m:*Pr` 里读到的属性。
    props: MathProps,
    /// 帧内未开帧的嵌套元素深度。
    other_depth: usize,
}

impl MathFrame {
    fn new(kind: Option<MathStruct>, slot: Option<MathSlot>) -> Self {
        MathFrame {
            kind,
            slot,
            items: Vec::new(),
            slots: Vec::new(),
            props: MathProps::default(),
            other_depth: 0,
        }
    }
}

/// 结构元素本地名 -> 结构种类。
fn math_struct_of(name: &[u8]) -> Option<MathStruct> {
    match name {
        b"f" => Some(MathStruct::Frac),
        b"sSup" => Some(MathStruct::Sup),
        b"sSub" => Some(MathStruct::Sub),
        b"sSubSup" => Some(MathStruct::SubSup),
        b"sPre" => Some(MathStruct::Pre),
        b"rad" => Some(MathStruct::Rad),
        b"d" => Some(MathStruct::Delim),
        b"nary" => Some(MathStruct::Nary),
        b"func" => Some(MathStruct::Func),
        b"limLow" => Some(MathStruct::LimLow),
        b"limUpp" => Some(MathStruct::LimUpp),
        b"acc" => Some(MathStruct::Acc),
        b"bar" => Some(MathStruct::Bar),
        b"m" => Some(MathStruct::Matrix),
        b"box" | b"borderBox" | b"phant" | b"groupChr" => Some(MathStruct::Wrapper),
        b"eqArr" => Some(MathStruct::EqArr),
        _ => None,
    }
}

/// 槽位元素本地名 -> 在给定结构里的槽位(不属于该结构的名字不算槽位)。
/// 第二项:槽位本身又是结构(只有矩阵的行 `m:mr`)。
fn math_slot_of(kind: MathStruct, name: &[u8]) -> Option<(MathSlot, Option<MathStruct>)> {
    use MathSlot as S;
    use MathStruct as K;
    let slot = match (kind, name) {
        (K::Frac, b"num") => S::Num,
        (K::Frac, b"den") => S::Den,
        (
            K::Sup
            | K::Sub
            | K::SubSup
            | K::Pre
            | K::Rad
            | K::Delim
            | K::Nary
            | K::Func
            | K::LimLow
            | K::LimUpp
            | K::Acc
            | K::Bar
            | K::Wrapper
            | K::EqArr,
            b"e",
        ) => S::Base,
        (K::Sup | K::SubSup | K::Pre | K::Nary, b"sup") => S::Sup,
        (K::Sub | K::SubSup | K::Pre | K::Nary, b"sub") => S::Sub,
        (K::Rad, b"deg") => S::Deg,
        (K::Func, b"fName") => S::FName,
        (K::LimLow | K::LimUpp, b"lim") => S::Lim,
        (K::Matrix, b"mr") => return Some((S::Row, Some(K::MatRow))),
        (K::MatRow, b"e") => S::Cell,
        _ => return None,
    };
    Some((slot, None))
}

// ------------------------------------------------------------------ 字符类与原子判定

fn is_open(c: char) -> bool {
    matches!(c, '(' | '[' | '{' | '⟨' | '⌊' | '⌈')
}

fn is_close(c: char) -> bool {
    matches!(c, ')' | ']' | '}' | '⟩' | '⌋' | '⌉')
}

/// 低优先级运算符 / 关系符 / 标点:两侧的项不会与之并成更大的原子,也不改变分式的结合。
fn is_loose(c: char) -> bool {
    matches!(
        c,
        '+' | '-'
            | '−'
            | '±'
            | '∓'
            | '='
            | '≠'
            | '<'
            | '>'
            | '≤'
            | '≥'
            | '≈'
            | '≡'
            | '∝'
            | '∼'
            | '≃'
            | '≅'
            | '→'
            | '←'
            | '↔'
            | '⇒'
            | '⇐'
            | '⇔'
            | '↦'
            | ','
            | ';'
            | ':'
            | '∈'
            | '∉'
            | '∋'
            | '⊂'
            | '⊃'
            | '⊆'
            | '⊇'
            | '∪'
            | '∩'
            | '∧'
            | '∨'
            | '|'
            | '∣'
            | '∥'
            | '…'
    )
}

/// 紧运算符:本身把两侧隔开(不必加空格),但与分式相邻会改变结合方式(属乘法语境)。
fn is_tight(c: char) -> bool {
    matches!(c, '^' | '_' | '/' | '·' | '⋅' | '×' | '÷' | '*' | '∘')
}

/// 结构右侧紧跟这些字符时,非原子的结构要整体加括号(`(x^2)!`)。
fn is_postfix(c: char) -> bool {
    matches!(c, '^' | '_' | '/' | '!' | '\'' | '′' | '″')
}

/// 组合重音符(常用区段):单字母原子允许带若干个。
fn is_combining(c: char) -> bool {
    matches!(c as u32, 0x300..=0x36F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

/// 拼接处是否要插空格(至少一侧是结构时才调用):一侧是空白 / 运算符 / 标点,或左开括号、右闭括号时不要。
fn needs_space(left: char, right: char) -> bool {
    let sep_left = left.is_whitespace() || is_loose(left) || is_tight(left) || is_open(left);
    let sep_right = right.is_whitespace()
        || is_loose(right)
        || is_tight(right)
        || is_close(right)
        || is_postfix(right);
    !(sep_left || sep_right)
}

/// 从 `s` 开头吃掉一个原子,返回剩余部分;开头不是原子返回 `None`。
fn take_atom(s: &str) -> Option<&str> {
    let mut chars = s.char_indices();
    let (_, first) = chars.next()?;
    if first.is_ascii_digit() || first == '.' {
        let end = s
            .char_indices()
            .find(|&(_, c)| !(c.is_ascii_digit() || c == '.'))
            .map_or(s.len(), |(i, _)| i);
        let num = &s[..end];
        let ok = num.bytes().filter(|&b| b == b'.').count() <= 1
            && num.bytes().any(|b| b.is_ascii_digit());
        return ok.then(|| &s[end..]);
    }
    if first.is_alphabetic() {
        let end = s
            .char_indices()
            .skip(1)
            .find(|&(_, c)| !is_combining(c))
            .map_or(s.len(), |(i, _)| i);
        return Some(&s[end..]);
    }
    if is_open(first) {
        // 括号组:配对到与开头匹配的闭括号为止,中途不能错配。
        let mut stack: Vec<char> = Vec::new();
        for (i, c) in s.char_indices() {
            if is_open(c) {
                stack.push(c);
            } else if is_close(c) {
                let open = stack.pop()?;
                if !brackets_match(open, c) {
                    return None;
                }
                if stack.is_empty() {
                    return Some(&s[i + c.len_utf8()..]);
                }
            }
        }
        return None;
    }
    None
}

fn brackets_match(open: char, close: char) -> bool {
    matches!(
        (open, close),
        ('(', ')') | ('[', ']') | ('{', '}') | ('⟨', '⟩') | ('⌊', '⌋') | ('⌈', '⌉')
    )
}

/// 规则 1:空串、单字母(可带组合重音符)、数字串、或被一对匹配括号完整包住。
fn is_atom(text: &str) -> bool {
    text.is_empty() || take_atom(text) == Some("")
}

/// 带上下标的原子(`x_i`、`x^2`、`x_i^2`、`(a)^n`):作被作用式时读者公认是一个整体。
fn is_scripted_atom(text: &str) -> bool {
    let Some(mut rest) = take_atom(text) else {
        return false;
    };
    while let Some(after) = rest.strip_prefix(['_', '^']) {
        match take_atom(after) {
            Some(r) => rest = r,
            None => return false,
        }
    }
    rest.is_empty()
}

/// `m:limLow` / `m:limUpp` 的底:运算符名(纯 ASCII 字母词,`lim` / `max` / `sup`)整体就是运算符,
/// 不加括号;其余同普通槽位。
fn limit_base(text: &str) -> String {
    if text.len() > 1 && text.bytes().all(|b| b.is_ascii_alphabetic()) {
        text.to_string()
    } else {
        wrap(text)
    }
}

/// 非原子加圆括号。
fn wrap(text: &str) -> String {
    if is_atom(text) {
        text.to_string()
    } else {
        format!("({text})")
    }
}

// ------------------------------------------------------------------ 项序列 -> 文字

/// 把一个项序列拼成文字,按规则 3 / 4 / 5 处理相邻项。线性:每项只扫一次。
fn render(items: &[Item]) -> String {
    // 每项之后(到下一个硬边界为止)第一个非空白字符:右邻判定用,一趟从右往左算出。
    let mut next_solid: Vec<Option<char>> = vec![None; items.len()];
    let mut carry = None;
    for (i, it) in items.iter().enumerate().rev() {
        next_solid[i] = carry;
        let text = match it {
            Item::Break => {
                carry = None;
                continue;
            }
            Item::Text(t) => t,
            Item::Struct(s) => &s.text,
        };
        if let Some(c) = text.chars().find(|c| !c.is_whitespace()) {
            carry = Some(c);
        }
    }
    let mut out = String::new();
    // 当前段(硬边界之间)里最后输出的字符 / 最后一个非空白字符。
    let mut last: Option<char> = None;
    let mut last_solid: Option<char> = None;
    let mut prev_struct = false;
    let mut pending_break = false;
    for (i, it) in items.iter().enumerate() {
        let piece = match it {
            Item::Break => {
                pending_break = true;
                last = None;
                last_solid = None;
                prev_struct = false;
                continue;
            }
            Item::Text(t) => {
                if t.is_empty() {
                    continue;
                }
                let space = prev_struct
                    && matches!((last, t.chars().next()), (Some(a), Some(b)) if needs_space(a, b));
                prev_struct = false;
                (t.clone(), space)
            }
            Item::Struct(s) => {
                if s.text.is_empty() {
                    continue;
                }
                let product_left = last_solid.is_some_and(|c| !(is_loose(c) || is_open(c)));
                let product_right = next_solid[i].is_some_and(|c| !(is_loose(c) || is_close(c)));
                let mut text = match &s.alt {
                    Some(alt)
                        if (s.group_left && product_left) || (s.group_right && product_right) =>
                    {
                        alt.clone()
                    }
                    _ => s.text.clone(),
                };
                let next_char = match items.get(i + 1) {
                    Some(Item::Text(t)) => t.chars().next(),
                    Some(Item::Struct(n)) => n.text.chars().next(),
                    _ => None,
                };
                let script_adjacent = last.is_some_and(|c| matches!(c, '^' | '_' | '/'))
                    || next_char.is_some_and(is_postfix);
                if script_adjacent && !is_atom(&text) {
                    text = format!("({text})");
                }
                let space =
                    matches!((last, text.chars().next()), (Some(a), Some(b)) if needs_space(a, b));
                prev_struct = true;
                (text, space)
            }
        };
        let (text, space) = piece;
        if pending_break && !out.is_empty() {
            out.push(' ');
        }
        pending_break = false;
        if space {
            out.push(' ');
        }
        out.push_str(&text);
        last = text.chars().next_back();
        if let Some(c) = text.chars().rev().find(|c| !c.is_whitespace()) {
            last_solid = Some(c);
        }
    }
    out
}

/// 往项序列末尾追加一项:相邻文字合并(定界符等也可能以文字形式进来)。
fn push_item(items: &mut Vec<Item>, item: Item) {
    match (items.last_mut(), item) {
        (_, Item::Text(t)) if t.is_empty() => {}
        (Some(Item::Text(prev)), Item::Text(t)) => prev.push_str(&t),
        (_, item) => items.push(item),
    }
}

/// 把一个结构帧收拢成项(0 项 = 所有槽位都为空;透明外壳 / 方程组展开成多项)。
fn linearize(kind: MathStruct, slots: Vec<(MathSlot, Vec<Item>)>, props: &MathProps) -> Vec<Item> {
    if matches!(kind, MathStruct::Wrapper | MathStruct::EqArr) {
        let mut out = Vec::new();
        for (_, items) in slots {
            if items.is_empty() {
                continue;
            }
            if kind == MathStruct::EqArr && !out.is_empty() {
                out.push(Item::Break);
            }
            for it in items {
                push_item(&mut out, it);
            }
        }
        return out;
    }
    let texts: Vec<(MathSlot, String)> =
        slots.iter().map(|(s, items)| (*s, render(items))).collect();
    if texts.iter().all(|(_, t)| t.is_empty()) {
        return Vec::new();
    }
    let slot = |want: MathSlot| {
        texts
            .iter()
            .find(|(s, _)| *s == want)
            .map_or("", |(_, t)| t.as_str())
    };
    let all = |want: MathSlot| -> Vec<&str> {
        texts
            .iter()
            .filter(|(s, _)| *s == want)
            .map(|(_, t)| t.as_str())
            .collect()
    };
    let chr = |default: &str| props.chr.clone().unwrap_or_else(|| default.to_string());
    // 被作用式(求和 / 积分 / 函数自变量):原子或带上下标的原子不加括号。
    let operand = |t: &str| {
        if is_atom(t) || is_scripted_atom(t) {
            t.to_string()
        } else {
            format!("({t})")
        }
    };
    let scripts = |out: &mut String| {
        if !slot(MathSlot::Sub).is_empty() {
            out.push('_');
            out.push_str(&wrap(slot(MathSlot::Sub)));
        }
        if !slot(MathSlot::Sup).is_empty() {
            out.push('^');
            out.push_str(&wrap(slot(MathSlot::Sup)));
        }
    };
    let plain = |text: String| vec![Item::Struct(StructOut::plain(text))];
    match kind {
        MathStruct::Frac => {
            let text = format!(
                "{}/{}",
                wrap(slot(MathSlot::Num)),
                wrap(slot(MathSlot::Den))
            );
            vec![Item::Struct(StructOut {
                alt: Some(format!("({text})")),
                text,
                group_left: true,
                group_right: true,
            })]
        }
        MathStruct::Sup => plain(format!(
            "{}^{}",
            wrap(slot(MathSlot::Base)),
            wrap(slot(MathSlot::Sup))
        )),
        MathStruct::Sub => plain(format!(
            "{}_{}",
            wrap(slot(MathSlot::Base)),
            wrap(slot(MathSlot::Sub))
        )),
        MathStruct::SubSup => plain(format!(
            "{}_{}^{}",
            wrap(slot(MathSlot::Base)),
            wrap(slot(MathSlot::Sub)),
            wrap(slot(MathSlot::Sup))
        )),
        MathStruct::Pre => {
            let mut pre = String::new();
            scripts(&mut pre);
            let base = wrap(slot(MathSlot::Base));
            if pre.is_empty() {
                plain(base)
            } else {
                plain(format!("({pre}){base}"))
            }
        }
        MathStruct::Rad => match slot(MathSlot::Deg) {
            "" => plain(format!("sqrt({})", slot(MathSlot::Base))),
            deg => plain(format!("root({},{})", wrap(deg), slot(MathSlot::Base))),
        },
        MathStruct::Delim => {
            let sep = match props.sep.as_deref() {
                None => "|",
                Some("") => " ",
                Some(s) => s,
            };
            let text = format!(
                "{}{}{}",
                props.beg.as_deref().unwrap_or("("),
                all(MathSlot::Base).join(sep),
                props.end.as_deref().unwrap_or(")")
            );
            // 两端都是非字母数字的定界符:自成一组,按文字拼接;否则按结构处理(两侧加空格)。
            let bracketed =
                |c: Option<char>| c.is_some_and(|c| !c.is_alphanumeric() && !c.is_whitespace());
            if bracketed(text.chars().next()) && bracketed(text.chars().next_back()) {
                vec![Item::Text(text)]
            } else {
                plain(text)
            }
        }
        MathStruct::Nary => {
            let mut text = chr("∫");
            scripts(&mut text);
            let body = slot(MathSlot::Base);
            if body.is_empty() {
                return plain(text);
            }
            text.push(' ');
            text.push_str(&operand(body));
            vec![Item::Struct(StructOut {
                alt: Some(format!("({text})")),
                text,
                group_left: false,
                group_right: true,
            })]
        }
        MathStruct::Func => {
            let (name, arg) = (slot(MathSlot::FName), slot(MathSlot::Base));
            if arg.is_empty() {
                plain(name.to_string())
            } else if name.is_empty() {
                plain(arg.to_string())
            } else if is_atom(arg) || is_scripted_atom(arg) {
                vec![Item::Struct(StructOut {
                    text: format!("{name} {arg}"),
                    alt: Some(format!("{name}({arg})")),
                    group_left: false,
                    group_right: true,
                })]
            } else {
                plain(format!("{name}({arg})"))
            }
        }
        MathStruct::LimLow => plain(format!(
            "{}_{}",
            limit_base(slot(MathSlot::Base)),
            wrap(slot(MathSlot::Lim))
        )),
        MathStruct::LimUpp => plain(format!(
            "{}^{}",
            limit_base(slot(MathSlot::Base)),
            wrap(slot(MathSlot::Lim))
        )),
        MathStruct::Acc => plain(format!("{}{}", wrap(slot(MathSlot::Base)), chr("\u{302}"))),
        MathStruct::Bar => {
            let word = if props.pos.as_deref() == Some("top") {
                "overline"
            } else {
                "underline"
            };
            plain(format!("{word}({})", slot(MathSlot::Base)))
        }
        MathStruct::Matrix => vec![Item::Text(format!("[{}]", all(MathSlot::Row).join(";")))],
        MathStruct::MatRow => plain(all(MathSlot::Cell).join(",")),
        MathStruct::Wrapper | MathStruct::EqArr => Vec::new(),
    }
}

/// 把结束的帧并入父帧:结构先线性化成项;槽位 -> 父结构的槽表;其余 -> 并入父帧的项序列。
fn math_merge(parent: &mut MathFrame, done: MathFrame) {
    let items = match done.kind {
        Some(kind) => {
            let mut items = linearize(kind, done.slots, &done.props);
            for it in done.items {
                push_item(&mut items, it);
            }
            items
        }
        None => done.items,
    };
    match (done.slot, parent.kind.is_some()) {
        (Some(slot), true) => parent.slots.push((slot, items)),
        _ => {
            for it in items {
                push_item(&mut parent.items, it);
            }
        }
    }
}

/// 记下 `m:*Pr` 里的 `m:val` 属性(`begChr` / `endChr` / `sepChr` / `chr` / `pos`):
/// 只在结构帧的属性子树内(`other_depth >= 1`)生效,先出现者为准。
fn math_capture_prop(top: &mut MathFrame, name: &[u8], e: &BytesStart) {
    if top.kind.is_none() || top.other_depth == 0 {
        return;
    }
    let slot = match name {
        b"begChr" => &mut top.props.beg,
        b"endChr" => &mut top.props.end,
        b"sepChr" => &mut top.props.sep,
        b"chr" => &mut top.props.chr,
        b"pos" => &mut top.props.pos,
        _ => return,
    };
    if slot.is_none() {
        *slot = Some(attr_of(e, b"val").unwrap_or_default());
    }
}

/// 解析 `m:oMath` / `m:oMathPara`:按文档顺序抽取其中所有文字元素(`m:t`,亦含 run 内偶见的
/// `w:t`),按模块文档的规则线性化成一个 `is_math` run。`m:oMathPara` 内多个 `m:oMath` 之间是硬边界。
/// 修订删除 `w:del` / `w:moveFrom` 子树按“接受修订”丢弃。已消费起始标签;抽不出文字时返回 `None`。
pub(super) fn parse_math<R: std::io::BufRead>(
    reader: &mut Reader<R>,
    container: &[u8],
) -> Option<TextRun> {
    let is_para = container == b"oMathPara";
    let mut stack = vec![MathFrame::new(None, None)];
    let mut struct_depth = 0u32;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                let name = name.as_slice();
                // 栈永不为空(根帧只在容器结束时才处理),下面的 last_mut 都成立。
                let Some(top) = stack.last_mut() else { break };
                math_capture_prop(top, name, &e);
                match name {
                    b"del" | b"moveFrom" => skip_element(reader),
                    b"t" => {
                        let t = read_text(reader);
                        push_item(&mut top.items, Item::Text(t));
                    }
                    _ if top.other_depth == 0 && top.kind.is_some() => {
                        // 结构帧的直接子元素:认得的槽位开新帧,其余当普通嵌套。
                        match top.kind.and_then(|k| math_slot_of(k, name)) {
                            Some((slot, kind)) => {
                                if kind.is_some() {
                                    struct_depth += 1;
                                }
                                stack.push(MathFrame::new(kind, Some(slot)));
                            }
                            None => top.other_depth += 1,
                        }
                    }
                    // 容器 / 槽位帧(`kind` 为 `None`)里不论隔了几层透明外壳(`m:oMath` /
                    // `m:e` 等)都认结构;外壳只记 `other_depth`。
                    _ if top.kind.is_none() && struct_depth < MAX_NEST_DEPTH => {
                        match math_struct_of(name) {
                            Some(kind) => {
                                struct_depth += 1;
                                stack.push(MathFrame::new(Some(kind), None));
                            }
                            None => {
                                if name == b"oMath" && is_para && !top.items.is_empty() {
                                    top.items.push(Item::Break);
                                }
                                top.other_depth += 1;
                            }
                        }
                    }
                    _ => {
                        if name == b"oMath" && is_para && !top.items.is_empty() {
                            top.items.push(Item::Break);
                        }
                        top.other_depth += 1;
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                if let Some(top) = stack.last_mut() {
                    math_capture_prop(top, &name, &e);
                }
            }
            Ok(Event::End(_)) => {
                let Some(top) = stack.last_mut() else { break };
                if top.other_depth > 0 {
                    top.other_depth -= 1;
                } else if stack.len() == 1 {
                    break; // 容器自身结束。
                } else if let Some(done) = stack.pop() {
                    if done.kind.is_some() {
                        struct_depth -= 1;
                    }
                    if let Some(parent) = stack.last_mut() {
                        math_merge(parent, done);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    // 畸形输入(Eof 时栈未回到根):自内向外把未闭合的帧并入父帧,文字不丢。
    while stack.len() > 1 {
        let Some(done) = stack.pop() else { break };
        if let Some(parent) = stack.last_mut() {
            math_merge(parent, done);
        }
    }
    let text = stack.pop().map(|f| render(&f.items)).unwrap_or_default();
    if text.is_empty() {
        return None;
    }
    let mut run = TextRun::from_text(&text);
    run.is_math = true;
    Some(run)
}
