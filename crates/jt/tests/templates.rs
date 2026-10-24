use std::collections::BTreeMap;

use jt::{Raw, ToJson};

/// Renders and checks the output is valid JSON before comparing.
fn render(value: &impl ToJson) -> String {
    let s = jt::to_string(value);
    if let Err(e) = serde_json::from_str::<serde_json::Value>(&s) {
        panic!("invalid JSON {s:?}: {e}");
    }
    s
}

jt::template! {
    pub fn Simple(name: &str, age: i64, active: bool) -> Json {
        "name": name,
        "age": age,
        "active": active,
        "type": "user",
        "score": 42,
        "ratio": -1.5,
        "verified": true,
        "metadata": null,
    }

    pub fn Shorthand(id: u32, name: &str) -> Json {
        id,
        name,
        r#type: "t",
    }

    pub fn Optional(a: Option<&str>, b: Option<i32>, c: Option<String>) -> Json {
        "a"?: a,
        "b"?: b,
        "c"?: c,
    }

    pub fn Cond(x: bool, y: bool) -> Json {
        if x { "x": 1 }
        if y { "y": 2, "yy": 3 } else { "noy": true }
        "z": 3,
    }

    pub fn LeadingCond(x: bool) -> Json {
        if x { "x": 1 }
        "always": 0,
    }

    pub fn OnlyCond(x: bool, y: bool) -> Json {
        if x { "x": 1 }
        if y { "y": 2 }
    }

    pub fn Loop(xs: &[i32]) -> Json {
        "xs": [ for x in xs { x } ],
        "doubled": [ for x in xs { x * 2, -x } ],
        "evens": [ for x in xs { if x % 2 == 0 { x } } ],
        "pre": [ 0, for x in xs { x } ],
        "post": [ for x in xs { x }, 99 ],
        "spread": [ ...xs, 7 ],
    }

    pub fn Keys(pairs: &[(&str, i32)]) -> Json {
        "fixed": 1,
        for (k, v) in pairs { [k]: v }
        [format!("k{}", pairs.len())]: "computed",
    }

    pub fn KeysOnly(pairs: &[(&str, i32)]) -> Json {
        for (k, v) in pairs { [k]: v }
    }

    pub fn Values(kind: u8, n: i32) -> Json {
        "role": if n > 0 { "positive" } else if n < 0 { "negative" } else { "zero" },
        "maybe": if n > 0 { n },
        "kind": match kind {
            0 => "zero",
            1 => { "one": [1] },
            _ => [kind, n],
        },
        match kind {
            0 => {}
            1 => { "is_one": true }
            _ => { "many": kind, "n": n }
        }
    }

    pub fn Lets(a: i32) -> Json {
        let b = a * 10;
        "a": a,
        let c = b + 1;
        "b": b,
        "c": c,
    }

    pub fn Base(id: u64) -> Json {
        "id": id,
        "kind": "base",
    }

    pub fn Extended(id: u64, extra: &BTreeMap<String, i32>, more: Option<&BTreeMap<String, i32>>) -> Json {
        ...Base(id),
        "own": true,
        ...extra,
        ...more,
    }

    pub fn SpreadFirst(extra: &BTreeMap<String, i32>) -> Json {
        ...extra,
        "after": 1,
    }

    pub fn Nested(title: &str, parent: &str) -> Json {
        "parent": { "database_id": parent },
        "properties": {
            "Name": { "title": [ { "text": { "content": title } } ] }
        },
        "empty_obj": {},
        "empty_arr": [],
    }

    pub fn Escaped(s: &str) -> Json {
        "quote\"key\n": "static \"value\"\t\u{1}",
        "dyn": s,
    }

    pub fn Composed(users: &[(&str, i64)]) -> Json {
        "users": [ for (n, a) in users { Simple(n, *a, a % 2 == 0) } ],
        "raw": Raw("{\"pre\":[1,2]}"),
    }

    pub fn Ids(ids: &[u64]) -> Json = [ for id in ids { id } ];

    pub fn Scalar(n: f64) -> Json = n;

    pub fn Generic<T: jt::ToJson>(items: &[T], label: &str) -> Json {
        label,
        items,
        "first": items.first(),
    }

    pub fn Empty() -> Json {}

    pub fn Calls(v: &[i32]) -> Json {
        "len": v.len(),
        "sum": v.iter().sum::<i32>(),
        "max": v.iter().copied().fold(i32::MIN, i32::max),
        "joined": v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(","),
        "turbofish": std::convert::identity::<i32>(1),
        "pair": (v.len(), "x"),
    }
}

#[test]
fn simple() {
    assert_eq!(
        render(&Simple("Ada", 36, true)),
        r#"{"name":"Ada","age":36,"active":true,"type":"user","score":42,"ratio":-1.5,"verified":true,"metadata":null}"#
    );
}

#[test]
fn shorthand_and_raw_identifiers() {
    assert_eq!(
        render(&Shorthand(1, "a")),
        r#"{"id":1,"name":"a","type":"t"}"#
    );
}

#[test]
fn optional_fields() {
    assert_eq!(render(&Optional(None, None, None)), "{}");
    assert_eq!(render(&Optional(Some("x"), None, None)), r#"{"a":"x"}"#);
    assert_eq!(render(&Optional(None, Some(2), None)), r#"{"b":2}"#);
    assert_eq!(
        render(&Optional(Some("x"), Some(2), Some("c".into()))),
        r#"{"a":"x","b":2,"c":"c"}"#
    );
    assert_eq!(
        render(&Optional(None, None, Some("c".into()))),
        r#"{"c":"c"}"#
    );
}

#[test]
fn conditional_entries() {
    assert_eq!(render(&Cond(false, false)), r#"{"noy":true,"z":3}"#);
    assert_eq!(render(&Cond(true, false)), r#"{"x":1,"noy":true,"z":3}"#);
    assert_eq!(render(&Cond(false, true)), r#"{"y":2,"yy":3,"z":3}"#);
    assert_eq!(render(&Cond(true, true)), r#"{"x":1,"y":2,"yy":3,"z":3}"#);
    assert_eq!(render(&LeadingCond(false)), r#"{"always":0}"#);
    assert_eq!(render(&LeadingCond(true)), r#"{"x":1,"always":0}"#);
    assert_eq!(render(&OnlyCond(false, false)), "{}");
    assert_eq!(render(&OnlyCond(true, false)), r#"{"x":1}"#);
    assert_eq!(render(&OnlyCond(false, true)), r#"{"y":2}"#);
    assert_eq!(render(&OnlyCond(true, true)), r#"{"x":1,"y":2}"#);
}

#[test]
fn loops() {
    assert_eq!(
        render(&Loop(&[])),
        r#"{"xs":[],"doubled":[],"evens":[],"pre":[0],"post":[99],"spread":[7]}"#
    );
    assert_eq!(
        render(&Loop(&[1, 2, 3])),
        r#"{"xs":[1,2,3],"doubled":[2,-1,4,-2,6,-3],"evens":[2],"pre":[0,1,2,3],"post":[1,2,3,99],"spread":[1,2,3,7]}"#
    );
    assert!(render(&Loop(&[1, 3])).contains(r#""evens":[]"#));
}

#[test]
fn computed_keys() {
    assert_eq!(render(&Keys(&[])), r#"{"fixed":1,"k0":"computed"}"#);
    assert_eq!(
        render(&Keys(&[("a", 1), ("b\"", 2)])),
        r#"{"fixed":1,"a":1,"b\"":2,"k2":"computed"}"#
    );
    assert_eq!(render(&KeysOnly(&[])), "{}");
    assert_eq!(render(&KeysOnly(&[("a", 1), ("b", 2)])), r#"{"a":1,"b":2}"#);
}

#[test]
fn value_control_flow() {
    assert_eq!(
        render(&Values(0, 0)),
        r#"{"role":"zero","maybe":null,"kind":"zero"}"#
    );
    assert_eq!(
        render(&Values(1, 5)),
        r#"{"role":"positive","maybe":5,"kind":{"one":[1]},"is_one":true}"#
    );
    assert_eq!(
        render(&Values(2, -3)),
        r#"{"role":"negative","maybe":null,"kind":[2,-3],"many":2,"n":-3}"#
    );
}

#[test]
fn let_statements() {
    assert_eq!(render(&Lets(2)), r#"{"a":2,"b":20,"c":21}"#);
}

#[test]
fn spreads() {
    let mut extra = BTreeMap::new();
    assert_eq!(
        render(&Extended(1, &extra, None)),
        r#"{"id":1,"kind":"base","own":true}"#
    );
    extra.insert("x".to_owned(), 1);
    extra.insert("y".to_owned(), 2);
    let more: BTreeMap<String, i32> = [("m".to_owned(), 3)].into();
    assert_eq!(
        render(&Extended(1, &extra, Some(&more))),
        r#"{"id":1,"kind":"base","own":true,"x":1,"y":2,"m":3}"#
    );
    assert_eq!(render(&SpreadFirst(&BTreeMap::new())), r#"{"after":1}"#);
    assert_eq!(render(&SpreadFirst(&extra)), r#"{"x":1,"y":2,"after":1}"#);
}

#[test]
fn nested_static() {
    assert_eq!(
        render(&Nested("T", "P")),
        r#"{"parent":{"database_id":"P"},"properties":{"Name":{"title":[{"text":{"content":"T"}}]}},"empty_obj":{},"empty_arr":[]}"#
    );
}

#[test]
fn escaping_matches_serde() {
    let s = "line\nbreak \"quoted\" back\\slash \u{7} ünïcødé 😀";
    let expected = format!(
        "{{{}:{},\"dyn\":{}}}",
        serde_json::to_string("quote\"key\n").unwrap(),
        serde_json::to_string("static \"value\"\t\u{1}").unwrap(),
        serde_json::to_string(s).unwrap()
    );
    assert_eq!(render(&Escaped(s)), expected);
}

#[test]
fn composition() {
    assert_eq!(
        render(&Composed(&[("a", 1), ("b", 2)])),
        r#"{"users":[{"name":"a","age":1,"active":false,"type":"user","score":42,"ratio":-1.5,"verified":true,"metadata":null},{"name":"b","age":2,"active":true,"type":"user","score":42,"ratio":-1.5,"verified":true,"metadata":null}],"raw":{"pre":[1,2]}}"#
    );
}

#[test]
fn non_object_templates() {
    assert_eq!(render(&Ids(&[])), "[]");
    assert_eq!(render(&Ids(&[1, 2])), "[1,2]");
    assert_eq!(render(&Scalar(0.5)), "0.5");
    assert_eq!(render(&Scalar(f64::NAN)), "null");
}

#[test]
fn generics() {
    assert_eq!(
        render(&Generic(&["a", "b"], "l")),
        r#"{"label":"l","items":["a","b"],"first":"a"}"#
    );
    assert_eq!(
        render(&Generic::<u8>(&[], "l")),
        r#"{"label":"l","items":[],"first":null}"#
    );
}

#[test]
fn empty_template() {
    assert_eq!(render(&Empty()), "{}");
}

#[test]
fn expressions() {
    assert_eq!(
        render(&Calls(&[3, 1, 2])),
        r#"{"len":3,"sum":6,"max":3,"joined":"3,1,2","turbofish":1,"pair":[3,"x"]}"#
    );
}

#[test]
fn struct_and_direct_write_agree() {
    let mut a = Vec::new();
    Simple::write(&mut a, "x", 1, false);
    let b = Simple {
        name: "x",
        age: 1,
        active: false,
    }
    .to_json_string();
    assert_eq!(String::from_utf8(a).unwrap(), b);
}

#[test]
fn every_output_path_agrees() {
    let t = Composed(&[("a", 1), ("b", 2), ("c", 3)]);
    let expected = jt::to_string(&t);

    assert_eq!(String::from_utf8(jt::to_vec(&t)).unwrap(), expected);
    assert_eq!(jt::to_pooled(&t).as_str(), expected);

    let mut buf = b"prefix".to_vec();
    jt::write_into(&mut buf, &t);
    assert_eq!(&buf[6..], expected.as_bytes());

    let mut s = String::new();
    t.to_json(&mut s);
    assert_eq!(s, expected);

    let mut sink = Vec::new();
    jt::to_writer(&mut sink, &t).unwrap();
    assert_eq!(sink, expected.as_bytes());

    // A one-byte chunk flushes at every checkpoint.
    let mut sink = Vec::new();
    let mut stream = jt::Stream::with_chunk(&mut sink, 1);
    t.to_json(&mut stream);
    stream.finish().unwrap();
    assert_eq!(sink, expected.as_bytes());
}

#[test]
fn stream_reports_write_errors() {
    struct Broken;
    impl std::io::Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("broken"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let err = jt::to_writer(Broken, &Ids(&[1, 2, 3])).unwrap_err();
    assert_eq!(err.to_string(), "broken");
}

#[test]
fn size_hint_learns() {
    let t = Composed(&[("a", 1); 50]);
    let first = jt::to_string(&t);
    assert!(t.json_size_hint() >= first.len());
}

#[test]
fn inline_json_macro() {
    let name = "Ada";
    let tags = vec!["a", "b"];
    let missing: Option<i32> = None;
    let s = jt::json!({
        "name": name,
        "tags": [ for t in &tags { t } ],
        "n"?: missing,
        "nested": { "ok": true },
    });
    assert_eq!(s, r#"{"name":"Ada","tags":["a","b"],"nested":{"ok":true}}"#);

    assert_eq!(jt::json!([1, "two", null]), r#"[1,"two",null]"#);
    assert_eq!(jt::json!(name), r#""Ada""#);

    let mut buf = Vec::new();
    jt::write_json!(buf, { "a": 1 });
    jt::write_json!(&mut buf, [2]);
    assert_eq!(buf, br#"{"a":1}[2]"#);
}

#[test]
fn std_types_match_serde() {
    let map: BTreeMap<u32, Vec<Option<f32>>> = [(1, vec![Some(1.5), None]), (2, vec![])]
        .into_iter()
        .collect();
    assert_eq!(jt::to_string(&map), serde_json::to_string(&map).unwrap());

    let values = (i64::MIN, u64::MAX, 1e300f64, 1e-7f64, 'x', "é", ());
    assert_eq!(
        jt::to_string(&values),
        serde_json::to_string(&values).unwrap()
    );

    let nested: Vec<Vec<&str>> = vec![vec![], vec!["\u{0}"]];
    assert_eq!(
        jt::to_string(&nested),
        serde_json::to_string(&nested).unwrap()
    );
}

#[test]
fn display_values() {
    let s =
        jt::json!({ "addr": jt::Display(std::net::Ipv4Addr::LOCALHOST), "q": jt::Display("a\"b") });
    assert_eq!(s, r#"{"addr":"127.0.0.1","q":"a\"b"}"#);
}
