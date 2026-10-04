//! 公式线性化的性质测试:**结构不同、数学含义不同的两个公式,线性化文本必须不同**(单射性的工程近似)。
//!
//! 生成器手写(不引第三方):原子集合 {`x`, `2`, `ab`, `2x`, `-1`, `α`} × 结构 {分式, 上标, 下标, 根号,
//! 求和, 函数, 定界符} 的一到两层嵌套,外加「项序列」(相邻组合)。所有公式放进同一个文档(每段一个
//! `m:oMath`),只解析一次。每个公式算一个**语义规范形**,规范形不同而输出相同即为撞车。
//!
//! 规范形只合并语义上真正相同的写法:相邻文字 run 合并(原文就是连续文字)、单元素序列拆开、
//! 槽位里缺省圆括号的定界符去掉一层(槽位本身就是一个分组,`(ab)` 作底与 `ab` 作底同义)。

use std::collections::HashMap;
use std::io::{Cursor, Write};

use doc_core::model::Block;
use doc_parse::parse_bytes;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const M_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/math";

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum F {
    T(String),
    Frac(Box<F>, Box<F>),
    Sup(Box<F>, Box<F>),
    Sub(Box<F>, Box<F>),
    Rad(Box<F>),
    Sum(Box<F>),
    Func(Box<F>),
    Delim(Box<F>),
    Seq(Vec<F>),
}

use F::*;

fn t(s: &str) -> F {
    T(s.to_string())
}

fn b(f: &F) -> Box<F> {
    Box::new(f.clone())
}

impl F {
    fn xml(&self) -> String {
        match self {
            T(s) => format!("<m:r><m:t>{s}</m:t></m:r>"),
            Frac(n, d) => format!(
                "<m:f><m:num>{}</m:num><m:den>{}</m:den></m:f>",
                n.xml(),
                d.xml()
            ),
            Sup(e, s) => format!(
                "<m:sSup><m:e>{}</m:e><m:sup>{}</m:sup></m:sSup>",
                e.xml(),
                s.xml()
            ),
            Sub(e, s) => format!(
                "<m:sSub><m:e>{}</m:e><m:sub>{}</m:sub></m:sSub>",
                e.xml(),
                s.xml()
            ),
            Rad(e) => format!("<m:rad><m:deg/><m:e>{}</m:e></m:rad>", e.xml()),
            Sum(e) => format!(
                r#"<m:nary><m:naryPr><m:chr m:val="∑"/></m:naryPr><m:sub><m:r><m:t>i=1</m:t></m:r></m:sub><m:sup><m:r><m:t>n</m:t></m:r></m:sup><m:e>{}</m:e></m:nary>"#,
                e.xml()
            ),
            Func(e) => format!(
                "<m:func><m:fName><m:r><m:t>sin</m:t></m:r></m:fName><m:e>{}</m:e></m:func>",
                e.xml()
            ),
            Delim(e) => format!("<m:d><m:e>{}</m:e></m:d>", e.xml()),
            Seq(v) => v.iter().map(F::xml).collect(),
        }
    }

    /// 语义规范形(见模块文档)。
    fn canon(&self) -> F {
        // 槽位:缺省圆括号的定界符去掉一层。
        let slot = |f: &F| match f.canon() {
            Delim(inner) => *inner,
            other => other,
        };
        match self {
            T(s) => T(s.clone()),
            Frac(n, d) => Frac(Box::new(slot(n)), Box::new(slot(d))),
            Sup(e, s) => Sup(Box::new(slot(e)), Box::new(slot(s))),
            Sub(e, s) => Sub(Box::new(slot(e)), Box::new(slot(s))),
            Rad(e) => Rad(Box::new(slot(e))),
            Sum(e) => Sum(Box::new(slot(e))),
            Func(e) => Func(Box::new(slot(e))),
            Delim(e) => Delim(Box::new(e.canon())),
            Seq(v) => {
                let mut out: Vec<F> = Vec::new();
                for f in v.iter().map(F::canon) {
                    let parts = match f {
                        Seq(inner) => inner,
                        other => vec![other],
                    };
                    for p in parts {
                        match (out.last_mut(), p) {
                            (Some(T(prev)), T(s)) => prev.push_str(&s),
                            (_, p) => out.push(p),
                        }
                    }
                }
                if out.len() == 1 {
                    out.pop().unwrap()
                } else {
                    Seq(out)
                }
            }
        }
    }
}

/// 生成公式集合。
fn formulas() -> Vec<F> {
    let atoms: Vec<F> = ["x", "2", "ab", "2x", "-1", "α"]
        .iter()
        .map(|s| t(s))
        .collect();
    let small: Vec<F> = vec![t("x"), t("2")];
    // 一层结构:槽位都取原子。
    let mut l1: Vec<F> = Vec::new();
    for a in &atoms {
        for c in &atoms {
            l1.push(Frac(b(a), b(c)));
            l1.push(Sup(b(a), b(c)));
            l1.push(Sub(b(a), b(c)));
        }
        l1.push(Rad(b(a)));
        l1.push(Sum(b(a)));
        l1.push(Func(b(a)));
        l1.push(Delim(b(a)));
    }
    // 两层结构:一个槽位取一层结构,其余槽位取 {x, 2}。
    let mut l2: Vec<F> = Vec::new();
    for s in &l1 {
        for a in &small {
            l2.push(Frac(b(s), b(a)));
            l2.push(Frac(b(a), b(s)));
            l2.push(Sup(b(s), b(a)));
            l2.push(Sup(b(a), b(s)));
            l2.push(Sub(b(s), b(a)));
            l2.push(Sub(b(a), b(s)));
        }
        l2.push(Rad(b(s)));
        l2.push(Sum(b(s)));
        l2.push(Func(b(s)));
        l2.push(Delim(b(s)));
    }
    // 项序列:原子 / 一层结构两两相邻,以及「原子 结构 原子」。
    let terms: Vec<F> = atoms.iter().chain(&l1).cloned().collect();
    let mut seqs: Vec<F> = Vec::new();
    for x in &terms {
        for y in &terms {
            seqs.push(Seq(vec![x.clone(), y.clone()]));
        }
    }
    for a in &atoms {
        for s in &l1 {
            for c in &atoms {
                seqs.push(Seq(vec![a.clone(), s.clone(), c.clone()]));
            }
        }
    }
    // 序列放进槽位(序列与嵌套交织):取一部分两项序列作分子 / 底 / 被加式 / 自变量。
    let mut in_slot: Vec<F> = Vec::new();
    for s in seqs.iter().step_by(37) {
        in_slot.push(Frac(b(s), b(&small[0])));
        in_slot.push(Sup(b(s), b(&small[1])));
        in_slot.push(Sum(b(s)));
        in_slot.push(Func(b(s)));
    }
    let mut all = atoms;
    all.extend(l1);
    all.extend(l2);
    all.extend(seqs);
    all.extend(in_slot);
    all
}

/// 把所有公式放进一个文档(每段一个公式),解析一次,返回每段的公式文字。
fn linearize_all(fs: &[F]) -> Vec<String> {
    let body: String = fs
        .iter()
        .map(|f| format!("<w:p><m:oMath>{}</m:oMath></w:p>", f.xml()))
        .collect();
    let xml = format!(
        r#"<w:document xmlns:w="{W_NS}" xmlns:m="{M_NS}"><w:body>{body}</w:body></w:document>"#
    );
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("word/document.xml", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(xml.as_bytes()).unwrap();
    let doc = parse_bytes(&zip.finish().unwrap().into_inner())
        .expect("parse")
        .document;
    let out: Vec<String> = doc
        .body
        .iter()
        .map(|blk| match blk {
            Block::Paragraph(p) => p.runs.iter().map(|r| r.text()).collect(),
            _ => panic!("expected paragraph"),
        })
        .collect();
    assert_eq!(out.len(), fs.len());
    out
}

#[test]
fn structurally_different_formulas_linearize_differently() {
    let fs = formulas();
    let texts = linearize_all(&fs);
    // 输出 -> 第一个产出它的规范形;规范形不同而输出相同即撞车。
    let mut seen: HashMap<&str, (F, &F)> = HashMap::new();
    let mut collisions = Vec::new();
    for (f, text) in fs.iter().zip(&texts) {
        let canon = f.canon();
        match seen.get(text.as_str()) {
            Some((c, first)) if *c != canon => {
                collisions.push(format!("{text:?}\n    {first:?}\n    {f:?}"));
            }
            Some(_) => {}
            None => {
                seen.insert(text, (canon, f));
            }
        }
    }
    let n = fs.len();
    eprintln!(
        "math_props: {n} 个公式,{} 个不同规范形,两两比较 {} 对",
        {
            let mut c: Vec<F> = fs.iter().map(F::canon).collect();
            c.sort_by_key(|f| format!("{f:?}"));
            c.dedup();
            c.len()
        },
        n * (n - 1) / 2
    );
    assert!(
        collisions.is_empty(),
        "{} 处撞车,前 20 处:\n{}",
        collisions.len(),
        collisions
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
