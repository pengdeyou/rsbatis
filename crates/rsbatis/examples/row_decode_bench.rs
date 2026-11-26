//! 解码一行的成本：8 个字段，每个都按列名查找。

use rsbatis::{Entity, FromRow, Row, Value};
use std::time::Instant;

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "wide")]
struct Wide {
    c0: i64,
    c1: String,
    c2: i64,
    c3: f64,
    c4: bool,
    c5: Option<i64>,
    c6: i64,
    c7: String,
}

fn main() {
    let names: Vec<String> = (0..8).map(|i| format!("c{i}")).collect();
    let row = Row::new(
        names,
        vec![
            Value::I64(42),
            Value::Text("hello".into()),
            Value::I64(7),
            Value::F64(1.5),
            Value::Bool(true),
            Value::Null,
            Value::I64(0),
            Value::Text("world".into()),
        ],
    );

    let n = 200_000usize;
    for _ in 0..1000 {
        std::hint::black_box(Wide::from_row(&row).unwrap());
    }

    let t = Instant::now();
    let mut sink = 0usize;
    for _ in 0..n {
        let w = Wide::from_row(&row).unwrap();
        sink += w.c0 as usize;
    }
    let e = t.elapsed();
    println!(
        "Wide::from_row (8 cols) x{n}: {:.1} ns/row, sink={sink}",
        e.as_nanos() as f64 / n as f64
    );

    // 渲染一份 SQL（含 8 列 + 一个条件）
    use rsbatis::{filter, select, Dialect};
    let stmt = select::<Wide>().filter(filter!(Wide.c0 >= 10));
    for _ in 0..1000 {
        std::hint::black_box(stmt.build(Dialect::Sqlite));
    }
    let t = Instant::now();
    let mut sink2 = 0usize;
    for _ in 0..n {
        let sql = stmt.build(Dialect::Sqlite);
        sink2 += sql.text.len();
    }
    let e = t.elapsed();
    println!(
        "build (8 cols + 1 predicate) x{n}: {:.1} ns/row, sink={sink2}",
        e.as_nanos() as f64 / n as f64
    );
}
