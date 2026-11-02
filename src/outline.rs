//! An outline of a JSON document that is not a table: `cargo metadata`, `terraform show -json`,
//! an API response with a few big records, a lock file.
//!
//! A document like that has no rows to count. What the model needs from it is its shape: which
//! keys there are, how big each part is, how long the arrays are, and which fields the records in
//! an array have. The outline gives that and points at the file for the values.
//!
//! The shape of an array is the merge of the shapes of all its elements, so a key that only some
//! records have is marked `?`, and a field that is sometimes `null` reads `str|null`.

use crate::lean::shell_quote;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
enum Shape {
    Empty,
    Null,
    Bool,
    Int,
    Num,
    Str {
        longest: usize,
    },
    Arr {
        min: usize,
        max: usize,
        elem: Box<Shape>,
    },
    Obj {
        count: usize,
        fields: Vec<Field>,
    },
    Mixed(Vec<Shape>),
}

#[derive(Clone, Debug, PartialEq)]
struct Field {
    name: String,
    shape: Shape,
    /// In how many of the merged objects the key occurs.
    seen: usize,
}

fn shape_of(v: &Value) -> Shape {
    match v {
        Value::Null => Shape::Null,
        Value::Bool(_) => Shape::Bool,
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                Shape::Int
            } else {
                Shape::Num
            }
        }
        Value::String(s) => Shape::Str {
            longest: s.chars().count(),
        },
        Value::Array(a) => {
            let elem = a.iter().map(shape_of).fold(Shape::Empty, merge);
            Shape::Arr {
                min: a.len(),
                max: a.len(),
                elem: Box::new(elem),
            }
        }
        Value::Object(m) => Shape::Obj {
            count: 1,
            fields: m
                .iter()
                .map(|(k, x)| Field {
                    name: k.clone(),
                    shape: shape_of(x),
                    seen: 1,
                })
                .collect(),
        },
    }
}

/// The kind of a shape, to tell which shapes can be merged into one.
fn kind(s: &Shape) -> u8 {
    match s {
        Shape::Empty => 0,
        Shape::Null => 1,
        Shape::Bool => 2,
        Shape::Int | Shape::Num => 3,
        Shape::Str { .. } => 4,
        Shape::Arr { .. } => 5,
        Shape::Obj { .. } => 6,
        Shape::Mixed(_) => 7,
    }
}

fn merge(a: Shape, b: Shape) -> Shape {
    match (a, b) {
        (Shape::Empty, x) | (x, Shape::Empty) => x,
        (Shape::Int, Shape::Num) | (Shape::Num, Shape::Int) | (Shape::Num, Shape::Num) => {
            Shape::Num
        }
        (Shape::Str { longest: x }, Shape::Str { longest: y }) => Shape::Str { longest: x.max(y) },
        (
            Shape::Arr {
                min: a0,
                max: a1,
                elem: ea,
            },
            Shape::Arr {
                min: b0,
                max: b1,
                elem: eb,
            },
        ) => Shape::Arr {
            min: a0.min(b0),
            max: a1.max(b1),
            elem: Box::new(merge(*ea, *eb)),
        },
        (
            Shape::Obj {
                count: ca,
                fields: mut fa,
            },
            Shape::Obj {
                count: cb,
                fields: fb,
            },
        ) => {
            for f in fb {
                match fa.iter_mut().find(|x| x.name == f.name) {
                    Some(x) => {
                        x.shape = merge(std::mem::replace(&mut x.shape, Shape::Empty), f.shape);
                        x.seen += f.seen;
                    }
                    None => fa.push(f),
                }
            }
            Shape::Obj {
                count: ca + cb,
                fields: fa,
            }
        }
        (Shape::Mixed(mut xs), y) => {
            add_variant(&mut xs, y);
            Shape::Mixed(xs)
        }
        (x, Shape::Mixed(ys)) => ys.into_iter().fold(x, merge),
        (x, y) if kind(&x) == kind(&y) => x,
        (x, y) => {
            let mut xs = vec![x];
            add_variant(&mut xs, y);
            Shape::Mixed(xs)
        }
    }
}

fn add_variant(xs: &mut Vec<Shape>, y: Shape) {
    match xs.iter().position(|x| kind(x) == kind(&y)) {
        Some(i) => {
            let old = std::mem::replace(&mut xs[i], Shape::Empty);
            xs[i] = merge(old, y);
        }
        None => xs.push(y),
    }
}

/// How much detail to show.
#[derive(Clone, Copy)]
struct Limits {
    depth: usize,
    keys: usize,
    top_keys: usize,
}

const LIMITS: [Limits; 4] = [
    Limits {
        depth: 3,
        keys: 10,
        top_keys: 30,
    },
    Limits {
        depth: 2,
        keys: 8,
        top_keys: 24,
    },
    Limits {
        depth: 2,
        keys: 5,
        top_keys: 16,
    },
    Limits {
        depth: 1,
        keys: 4,
        top_keys: 10,
    },
];

fn render(s: &Shape, depth: usize, lim: Limits) -> String {
    match s {
        Shape::Empty => "empty".into(),
        Shape::Null => "null".into(),
        Shape::Bool => "bool".into(),
        Shape::Int => "int".into(),
        Shape::Num => "num".into(),
        Shape::Str { longest } if *longest > 200 => format!("str(<={longest})"),
        Shape::Str { .. } => "str".into(),
        Shape::Arr { min, max, elem } => {
            let size = if min == max {
                format!("{min}")
            } else {
                format!("{min}..{max}")
            };
            if **elem == Shape::Empty {
                format!("array[{size}]")
            } else {
                // An array is not a level of nesting: its records are as deep as the array.
                format!("array[{size}] of {}", render(elem, depth, lim))
            }
        }
        Shape::Obj { count, fields } => {
            if depth > lim.depth {
                return format!("{{...{} keys}}", fields.len());
            }
            // An object whose keys are data (feature names, ids): most keys occur in one record
            // only. List a few and say how many.
            let rare = fields.iter().filter(|f| f.seen * 2 <= *count).count();
            if *count > 2 && fields.len() > lim.keys && rare * 10 >= fields.len() * 7 {
                let names: Vec<&str> = fields.iter().take(4).map(|f| f.name.as_str()).collect();
                let merged = fields
                    .iter()
                    .map(|f| f.shape.clone())
                    .fold(Shape::Empty, merge);
                return format!(
                    "map of {} keys ({}, ...) to {}",
                    fields.len(),
                    names.join(", "),
                    render(&merged, depth + 1, lim)
                );
            }
            let shown: Vec<String> = fields
                .iter()
                .take(lim.keys)
                .map(|f| {
                    let opt = if f.seen < *count { "?" } else { "" };
                    // `{root_module:{resources:[...]}}`: a wrapper with one key is not a level.
                    let child = if fields.len() == 1 { depth } else { depth + 1 };
                    format!("{}{opt}:{}", f.name, render(&f.shape, child, lim))
                })
                .collect();
            let more = if fields.len() > lim.keys {
                format!(", +{} keys", fields.len() - lim.keys)
            } else {
                String::new()
            };
            format!("{{{}{more}}}", shown.join(", "))
        }
        Shape::Mixed(xs) => xs
            .iter()
            .map(|x| render(x, depth, lim))
            .collect::<Vec<_>>()
            .join("|"),
    }
}

fn human(n: usize) -> String {
    if n >= 1 << 20 {
        format!("{:.1}MB", n as f64 / (1 << 20) as f64)
    } else if n >= 1024 {
        format!("{:.1}KB", n as f64 / 1024.0)
    } else {
        format!("{n}B")
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max - 1).collect();
        t.push('…');
        t
    }
}

/// A scalar short enough to show as it is.
fn short_scalar(v: &Value) -> Option<String> {
    match v {
        Value::Null => Some("null".into()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) if s.chars().count() <= 60 && !s.contains('\n') => Some(format!("{s:?}")),
        _ => None,
    }
}

/// jaq syntax for one key.
fn key_path(k: &str) -> String {
    if !k.is_empty()
        && k.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        format!(".{k}")
    } else {
        format!(".[{}]", serde_json::to_string(k).unwrap_or_default())
    }
}

/// The outline of a document. `bytes` is the size of the text it was read from; `file` is where
/// the full document is, so that the drill commands work.
pub fn outline(v: &Value, bytes: usize, file: Option<&str>, budget: usize) -> String {
    let mut last = String::new();
    for lim in LIMITS {
        last = outline_with(v, bytes, file, lim);
        if last.len().div_ceil(4) <= budget {
            break;
        }
    }
    last
}

fn outline_with(v: &Value, bytes: usize, file: Option<&str>, lim: Limits) -> String {
    let mut out = String::new();
    let mut biggest: Option<(String, usize, bool)> = None; // key, size, array of objects
    match v {
        Value::Object(m) => {
            out.push_str(&format!(
                "[dtk] json document: {}, an object with {} keys. This is an outline; the full document is on disk.\n",
                human(bytes),
                m.len()
            ));
            if let Some(p) = file {
                out.push_str(&format!("file: {}\n", shell_quote(p)));
            }
            for (k, x) in m.iter().take(lim.top_keys) {
                let size = serde_json::to_string(x).map(|s| s.len()).unwrap_or(0);
                let sz = if size >= 1024 {
                    format!(" ({})", human(size))
                } else {
                    String::new()
                };
                let desc = match (x, short_scalar(x)) {
                    (Value::Null, _) => "null".to_string(),
                    (_, Some(sc)) => format!("{} {}", render(&shape_of(x), 0, lim), sc),
                    (_, None) => render(&shape_of(x), 1, lim),
                };
                out.push_str(&format!("{}{sz}: {}\n", key_path(k), clip(&desc, 420)));
                if let Value::Array(a) = x {
                    if biggest.as_ref().map_or(true, |b| size > b.1) {
                        biggest = Some((
                            k.clone(),
                            size,
                            a.iter().all(Value::is_object) && !a.is_empty(),
                        ));
                    }
                }
            }
            if m.len() > lim.top_keys {
                out.push_str(&format!(
                    "... {} more keys: {}\n",
                    m.len() - lim.top_keys,
                    clip(
                        &m.keys()
                            .skip(lim.top_keys)
                            .take(12)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", "),
                        200
                    )
                ));
            }
        }
        Value::Array(a) => {
            out.push_str(&format!(
                "[dtk] json document: {}, an array of {}. This is an outline; the full document is on disk.\n",
                human(bytes),
                a.len()
            ));
            if let Some(p) = file {
                out.push_str(&format!("file: {}\n", shell_quote(p)));
            }
            out.push_str(&format!("{}\n", clip(&render(&shape_of(v), 0, lim), 900)));
            if let Some(Value::Object(first)) = a.first() {
                // The records are few and big. Show how big each one is, so the model can pick.
                let sizes: Vec<String> = a
                    .iter()
                    .take(8)
                    .enumerate()
                    .map(|(i, x)| {
                        let label = ["name", "id", "tag_name", "title"]
                            .iter()
                            .find_map(|k| first.get(*k).and(x.get(*k)).and_then(short_scalar))
                            .map(|l| format!(" {l}"))
                            .unwrap_or_default();
                        format!(
                            "[{i}]{label} {}",
                            human(serde_json::to_string(x).map(|s| s.len()).unwrap_or(0))
                        )
                    })
                    .collect();
                out.push_str(&format!("records: {}\n", sizes.join("; ")));
            }
            biggest = None;
        }
        _ => return String::new(),
    }
    // One example is enough: the outline already shows the keys and the shapes.
    let (dtk, f) = (
        crate::lean::command_name(),
        file.map_or_else(|| "<file>".to_string(), crate::lean::file_ref),
    );
    let example = match (&biggest, v) {
        (Some((k, _, _)), _) => format!("{dtk} get {f} '{}[0]'", key_path(k)),
        (None, Value::Array(_)) => format!("{dtk} get {f} '.[0]'"),
        _ => format!("{dtk} get {f} '. | keys'"),
    };
    out.push_str(&format!(
        "drill: {example}   (paths: .a.b[0]  .items[]  .items | length  .items | keys)"
    ));
    out.push('\n');
    out.trim_end().to_string()
}

/// True when the text is one JSON object or array that is worth an outline: it is not a table
/// (the caller has already tried), and it is bigger than a few lines.
pub fn parse_document(text: &str) -> Option<Value> {
    let t = text.trim();
    if !(t.starts_with('{') || t.starts_with('[')) || t.len() < 2000 {
        return None;
    }
    let v: Value = serde_json::from_str(t).ok()?;
    match &v {
        Value::Object(m) if !m.is_empty() => Some(v),
        Value::Array(a) if !a.is_empty() && a.iter().all(Value::is_object) => Some(v),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merges_records_so_that_optional_and_nullable_fields_show() {
        let v = json!({"items": [
            {"id": 1, "name": "a", "note": null, "tags": ["x"]},
            {"id": 2, "name": "b", "note": "hello", "extra": 1.5, "tags": []},
            {"id": 3, "name": "c", "note": null, "tags": ["y", "z"]},
        ]});
        let s = shape_of(&v);
        let Shape::Obj { fields, .. } = &s else {
            panic!()
        };
        let text = render(&fields[0].shape, 1, LIMITS[0]);
        assert!(
            text.starts_with("array[3] of {id:int, name:str, note:null|str"),
            "{text}"
        );
        assert!(text.contains("extra?:num"), "{text}");
        assert!(text.contains("tags:array[0..2] of str"), "{text}");
    }

    #[test]
    fn mixed_kinds_are_listed_once_each() {
        let v = json!([1, "a", 2, null, "b", 2.5]);
        assert_eq!(
            render(&shape_of(&v), 0, LIMITS[0]),
            "array[6] of num|str|null"
        );
    }

    #[test]
    fn deep_objects_stop_at_the_depth_limit() {
        let v = json!({"a": {"b": {"c": {"d": {"e": 1, "f": 2}, "x": 0}, "x": 0}, "x": 0}, "x": 0});
        let text = render(&shape_of(&v), 0, LIMITS[0]);
        assert!(text.contains("{...2 keys}"), "{text}");
        assert!(!text.contains("e:int"), "{text}");
    }

    #[test]
    fn the_outline_names_every_top_level_key_and_the_sizes() {
        let packages: Vec<Value> = (0..40)
            .map(|i| json!({"name": format!("crate{i}"), "version": "1.0.0", "deps": ["a", "b"], "license": null}))
            .collect();
        let v =
            json!({"packages": packages, "version": 1, "root": "/work", "empty": {}, "flag": true});
        let text = outline(&v, 9000, Some("/tmp/doc.json"), 2500);
        for want in [
            "an object with 5 keys",
            "file: /tmp/doc.json",
            ".packages (",
            "array[40] of {name:str, version:str, deps:array[2] of str, license:null}",
            ".version: int 1",
            ".root: str \"/work\"",
            ".flag: bool true",
            ".empty: {}",
            "dtk get /tmp/doc.json '.packages[0]'",
        ] {
            assert!(text.contains(want), "missing {want:?} in:\n{text}");
        }
    }

    #[test]
    fn a_few_big_records_are_listed_with_their_sizes() {
        let rec = |n: usize, name: &str| json!({"name": name, "body": "x".repeat(n), "id": n});
        let v = json!([rec(5000, "v1"), rec(9000, "v2"), rec(300, "v3")]);
        let text = outline(&v, 14_400, None, 2500);
        assert!(text.contains("an array of 3"), "{text}");
        assert!(text.contains("[0] \"v1\" "), "{text}");
        assert!(
            text.contains("[1] \"v2\" 8.8KB") || text.contains("[1] \"v2\" 9"),
            "{text}"
        );
    }

    #[test]
    fn long_strings_show_their_length_and_not_their_text() {
        let v = json!({"readme": "y".repeat(5000), "n": 1});
        let text = outline(&v, 5100, None, 2500);
        assert!(text.contains("str(<=5000)"), "{text}");
        assert!(!text.contains("yyyy"), "{text}");
    }

    #[test]
    fn documents_that_are_not_worth_an_outline_are_refused() {
        assert!(parse_document("{\"a\":1}").is_none(), "too small");
        assert!(
            parse_document(&format!("[{}]", "1,".repeat(2000) + "1")).is_none(),
            "numbers only"
        );
        assert!(parse_document("not json at all").is_none());
        let big = json!({"k": "v".repeat(3000)}).to_string();
        assert!(parse_document(&big).is_some());
    }
}
