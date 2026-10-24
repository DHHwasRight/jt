//! jt against serde_json on the same data, producing the same bytes.
//!
//! Every case asserts both outputs are identical before it is measured, so
//! the comparison is between two ways of writing one document, not two
//! documents.
//!
//! Run with `cargo bench -p jt --bench vs_serde`.

use std::hint::black_box;
use std::io;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use serde::Serialize;

// --- data -----------------------------------------------------------------

#[derive(Serialize)]
pub struct Address {
    street: String,
    city: String,
    zip: String,
    geo: [f64; 2],
}

#[derive(Serialize)]
pub struct User {
    id: u64,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    age: u32,
    score: f64,
    active: bool,
    tags: Vec<String>,
    bio: String,
    address: Address,
}

fn users(n: usize) -> Vec<User> {
    let words = [
        "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta",
    ];
    (0..n)
        .map(|i| User {
            id: 1_000_000 + i as u64 * 7919,
            name: format!("User Number {i}"),
            email: (i % 3 != 0).then(|| format!("user{i}@example.com")),
            age: 18 + (i % 60) as u32,
            score: i as f64 * 1.37 + 0.25,
            active: i % 2 == 0,
            tags: (0..i % 4)
                .map(|t| words[(i + t) % words.len()].to_owned())
                .collect(),
            bio: if i % 5 == 0 {
                format!("Says \"hi\"\nand\tleaves \\ {i}")
            } else {
                format!("A perfectly ordinary biography for user {i}, nothing to escape here.")
            },
            address: Address {
                street: format!("{} Main Street", i * 3),
                city: words[i % words.len()].to_owned(),
                zip: format!("{:05}", i * 13 % 100_000),
                geo: [48.8566 + i as f64 * 1e-4, 2.3522 - i as f64 * 1e-4],
            },
        })
        .collect()
}

#[derive(Serialize)]
struct SimpleSerde<'a> {
    name: &'a str,
    age: i64,
    active: bool,
    r#type: &'static str,
    score: i32,
    verified: bool,
    deleted: bool,
    metadata: Option<()>,
}

#[derive(Serialize)]
struct Text<'a> {
    content: &'a str,
}
#[derive(Serialize)]
struct TitlePart<'a> {
    text: Text<'a>,
}
#[derive(Serialize)]
struct Name<'a> {
    title: [TitlePart<'a>; 1],
}
#[derive(Serialize)]
struct Properties<'a> {
    #[serde(rename = "Name")]
    name: Name<'a>,
}
#[derive(Serialize)]
struct Parent<'a> {
    database_id: &'a str,
}
#[derive(Serialize)]
struct CreatePageSerde<'a> {
    parent: Parent<'a>,
    properties: Properties<'a>,
}

#[derive(Serialize)]
struct Doc<'a> {
    body: &'a str,
}

// --- templates ------------------------------------------------------------

jt::template! {
    pub fn UserJson(u: &User) -> Json {
        "id": u.id,
        "name": u.name,
        "email"?: u.email,
        "age": u.age,
        "score": u.score,
        "active": u.active,
        "tags": u.tags,
        "bio": u.bio,
        "address": {
            "street": u.address.street,
            "city": u.address.city,
            "zip": u.address.zip,
            "geo": u.address.geo,
        },
    }

    pub fn UsersJson(users: &[User]) -> Json = [ for u in users { UserJson(u) } ];

    pub fn Simple(name: &str, age: i64, active: bool) -> Json {
        "name": name,
        "age": age,
        "active": active,
        "type": "user",
        "score": 42,
        "verified": true,
        "deleted": false,
        "metadata": null,
    }

    pub fn CreatePage(parent_id: &str, title: &str) -> Json {
        "parent": { "database_id": parent_id },
        "properties": {
            "Name": { "title": [ { "text": { "content": title } } ] }
        },
    }

    pub fn DocJson(body: &str) -> Json { "body": body }
}

/// A writer that copies what it is given, as a socket copies into the
/// kernel. `io::sink()` would discard bytes without touching them, which makes
/// many tiny writes look free.
struct Socket {
    scratch: Vec<u8>,
}

impl Socket {
    fn new() -> Self {
        Socket {
            scratch: Vec::with_capacity(64 * 1024),
        }
    }
}

impl io::Write for Socket {
    #[inline(never)]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.scratch.len() + buf.len() > self.scratch.capacity() {
            self.scratch.clear();
        }
        self.scratch.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// --- cases ----------------------------------------------------------------

fn same(jt: &str, serde: &str) {
    assert_eq!(jt, serde, "jt and serde_json disagree");
}

fn small(c: &mut Criterion) {
    let mut g = c.benchmark_group("small");

    let serde_value = SimpleSerde {
        name: "Ada Lovelace",
        age: 36,
        active: true,
        r#type: "user",
        score: 42,
        verified: true,
        deleted: false,
        metadata: None,
    };
    same(
        &jt::to_string(&Simple("Ada Lovelace", 36, true)),
        &serde_json::to_string(&serde_value).unwrap(),
    );
    g.bench_function("flat/serde_json", |b| {
        b.iter(|| serde_json::to_string(black_box(&serde_value)).unwrap())
    });
    g.bench_function("flat/jt", |b| {
        b.iter(|| {
            jt::to_string(&Simple(
                black_box("Ada Lovelace"),
                black_box(36),
                black_box(true),
            ))
        })
    });

    let page = CreatePageSerde {
        parent: Parent {
            database_id: "d9824bdc-8445-4327-be8b-5b47500af6ce",
        },
        properties: Properties {
            name: Name {
                title: [TitlePart {
                    text: Text {
                        content: "Tuscan kale",
                    },
                }],
            },
        },
    };
    same(
        &jt::to_string(&CreatePage(
            "d9824bdc-8445-4327-be8b-5b47500af6ce",
            "Tuscan kale",
        )),
        &serde_json::to_string(&page).unwrap(),
    );
    g.bench_function("nested/serde_json", |b| {
        b.iter(|| serde_json::to_string(black_box(&page)).unwrap())
    });
    g.bench_function("nested/jt", |b| {
        b.iter(|| {
            jt::to_string(&CreatePage(
                black_box("d9824bdc-8445-4327-be8b-5b47500af6ce"),
                black_box("Tuscan kale"),
            ))
        })
    });
    g.finish();
}

fn records(c: &mut Criterion) {
    let mut g = c.benchmark_group("users");
    for n in [10usize, 1000] {
        let data = users(n);
        let expected = serde_json::to_string(&data).unwrap();
        same(&jt::to_string(&UsersJson(&data)), &expected);
        g.throughput(Throughput::Bytes(expected.len() as u64));

        g.bench_with_input(
            BenchmarkId::new("to_string/serde_json", n),
            &data,
            |b, d| b.iter(|| serde_json::to_string(black_box(d)).unwrap()),
        );
        g.bench_with_input(BenchmarkId::new("to_string/jt", n), &data, |b, d| {
            b.iter(|| jt::to_string(&UsersJson(black_box(d))))
        });

        // A reused buffer: what a server with a buffer per worker does.
        g.bench_with_input(
            BenchmarkId::new("reused_buf/serde_json", n),
            &data,
            |b, d| {
                let mut buf = Vec::with_capacity(expected.len());
                b.iter(|| {
                    buf.clear();
                    serde_json::to_writer(&mut buf, black_box(d)).unwrap();
                    black_box(buf.len())
                })
            },
        );
        g.bench_with_input(BenchmarkId::new("reused_buf/jt", n), &data, |b, d| {
            let mut buf = Vec::with_capacity(expected.len());
            b.iter(|| {
                buf.clear();
                UsersJson::write(&mut buf, black_box(d));
                black_box(buf.len())
            })
        });
        g.bench_with_input(BenchmarkId::new("pooled/jt", n), &data, |b, d| {
            b.iter(|| black_box(jt::to_pooled(&UsersJson(black_box(d)))).len())
        });

        // Streaming into an `io::Write`. serde_json writes every token
        // separately, so it is measured both bare and behind a `BufWriter`.
        g.bench_with_input(BenchmarkId::new("writer/serde_json", n), &data, |b, d| {
            let mut socket = Socket::new();
            b.iter(|| serde_json::to_writer(&mut socket, black_box(d)).unwrap())
        });
        g.bench_with_input(
            BenchmarkId::new("writer_bufwriter/serde_json", n),
            &data,
            |b, d| {
                let mut socket = Socket::new();
                b.iter(|| {
                    let mut w = io::BufWriter::with_capacity(8 * 1024, &mut socket);
                    serde_json::to_writer(&mut w, black_box(d)).unwrap();
                    io::Write::flush(&mut w).unwrap();
                })
            },
        );
        g.bench_with_input(BenchmarkId::new("writer/jt", n), &data, |b, d| {
            let mut socket = Socket::new();
            b.iter(|| jt::to_writer(&mut socket, &UsersJson(black_box(d))).unwrap())
        });
    }
    g.finish();
}

fn strings(c: &mut Criterion) {
    let mut g = c.benchmark_group("strings");
    let clean: String = "The quick brown fox jumps over the lazy dog. ".repeat(256);
    let dirty: String = "He said \"hi\"\n\tand left a \\ behind. ".repeat(256);
    let unicode: String = "Ünïcödé text — ≈ 😀 ".repeat(256);
    for (name, text) in [
        ("clean", &clean),
        ("escapes", &dirty),
        ("unicode", &unicode),
    ] {
        let doc = Doc { body: text };
        same(
            &jt::to_string(&DocJson(text)),
            &serde_json::to_string(&doc).unwrap(),
        );
        g.throughput(Throughput::Bytes(text.len() as u64));
        g.bench_function(BenchmarkId::new("serde_json", name), |b| {
            b.iter(|| serde_json::to_string(black_box(&doc)).unwrap())
        });
        g.bench_function(BenchmarkId::new("jt", name), |b| {
            b.iter(|| jt::to_string(&DocJson(black_box(text))))
        });
    }
    g.finish();
}

criterion_group!(benches, small, records, strings);
criterion_main!(benches);
