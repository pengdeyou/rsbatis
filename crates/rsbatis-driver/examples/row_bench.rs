//! 单行结果构造的成本：`Row::new` 会克隆列名。
//!
//! 一个 8 列的查询解 1000 行，就是 8000 次 `String` 分配——这部分与数据量无关，
//! 纯属每行重复搬运同一份列名。这里量的就是它。

use rsbatis_core::Value;
use rsbatis_driver::Row;
use std::time::Instant;

fn main() {
    let names: Vec<String> = (0..8).map(|i| format!("column_{i}")).collect();
    let vals: Vec<Value> = vec![
        Value::I64(42),
        Value::Text("hello".into()),
        Value::I64(7),
        Value::F64(1.5),
        Value::Bool(true),
        Value::Null,
        Value::I64(0),
        Value::Text("world".into()),
    ];

    let n = 200_000usize;

    // 预热，让分配器先热起来
    for _ in 0..1000 {
        let r = Row::new(names.clone(), vals.clone());
        std::hint::black_box(r.columns().len());
    }

    let t = Instant::now();
    let mut sink = 0usize;
    for _ in 0..n {
        let r = Row::new(names.clone(), vals.clone());
        sink += r.columns().len();
    }
    let e = t.elapsed();
    println!(
        "Row::new x{n}: {:.1} ns/row, sink={sink}",
        e.as_nanos() as f64 / n as f64
    );


    // 驱动真实路径：列名共享一份，值是新构造的（不克隆）
    let shared: std::sync::Arc<[String]> = std::sync::Arc::from(names.clone());
    let t = Instant::now();
    let mut sink = 0usize;
    for _ in 0..n {
        let mut v: Vec<Value> = Vec::with_capacity(8);
        v.push(Value::I64(42));
        v.push(Value::Text("hello".to_string()));
        v.push(Value::I64(7));
        v.push(Value::F64(1.5));
        v.push(Value::Bool(true));
        v.push(Value::Null);
        v.push(Value::I64(0));
        v.push(Value::Text("world".to_string()));
        let r = Row::from_shared(std::sync::Arc::clone(&shared), v);
        sink += r.columns().len();
    }
    let e = t.elapsed();
    println!(
        "Row::from_shared(real path) x{n}: {:.1} ns/row, sink={sink}",
        e.as_nanos() as f64 / n as f64
    );

    // 旧路径的等价：每行 `names.clone()` + `new`
    let t = Instant::now();
    let mut sink = 0usize;
    for _ in 0..n {
        let mut v: Vec<Value> = Vec::with_capacity(8);
        v.push(Value::I64(42));
        v.push(Value::Text("hello".to_string()));
        v.push(Value::I64(7));
        v.push(Value::F64(1.5));
        v.push(Value::Bool(true));
        v.push(Value::Null);
        v.push(Value::I64(0));
        v.push(Value::Text("world".to_string()));
        let r = Row::new(names.clone(), v);
        sink += r.columns().len();
    }
    let e = t.elapsed();
    println!(
        "Row::new(names.clone, fresh values) x{n}: {:.1} ns/row, sink={sink}",
        e.as_nanos() as f64 / n as f64
    );
}
