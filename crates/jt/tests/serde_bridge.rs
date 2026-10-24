#![cfg(feature = "serde")]
#[derive(serde::Serialize)]
struct P {
    x: i32,
    s: &'static str,
}

jt::template! {
    pub fn Wrap(p: &P) -> Json { "p": jt::Serde(p), "n": 1 }
}

#[test]
fn serde_bridge() {
    assert_eq!(
        jt::to_string(&Wrap(&P { x: 1, s: "a\"" })),
        r#"{"p":{"x":1,"s":"a\""},"n":1}"#
    );
}
