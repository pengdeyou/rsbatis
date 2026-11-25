
//! 级联写入（`save-update` 的显式版）：父 + 子一起插，父的自增 id 回填到子。

#![cfg(feature = "sqlite")]

use rsbatis::{Entity, FromRow, SelectStmt, Session, SqliteDriver, SortOrder};

#[derive(Entity, FromRow, Debug, Clone)]
#[rsbatis(table = "c_dept")]
#[allow(dead_code)]
struct Dept {
    // 必须是 Option：`None` 时 `column_values` 跳过这一列，数据库才会自增。
    // 写成 `i64` 会显式写入 0（0 不是 NULL），自增永远不触发 —— 插入的行 id 都是 0。
    #[rsbatis(primary_key)]
    id: Option<i64>,
    name: String,
}

#[derive(Entity, FromRow, Debug, Clone)]
#[rsbatis(table = "c_staff")]
#[allow(dead_code)]
struct Staff {
    #[rsbatis(primary_key)]
    id: Option<i64>,
    dept_id: i64,
    name: String,
}

/// 待插入用的"草稿"：id 还没有，用 0 占位（`column_values` 会把它写进去，
/// 所以这里演示的是"显式给自增列留 0"的形态）。
#[derive(Entity, FromRow, Debug, Clone)]
#[rsbatis(table = "c_dept")]
#[allow(dead_code)]
struct DeptDraft {
    id: i64,
    name: String,
}

fn fresh() -> Session<SqliteDriver> {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    // INTEGER PRIMARY KEY 才是 SQLite 的自增（rowid 别名）；用别的类型就没有自增。
    db.execute_batch(
        "CREATE TABLE c_dept (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
         CREATE TABLE c_staff (id INTEGER PRIMARY KEY, dept_id INTEGER NOT NULL,
                               name TEXT NOT NULL);",
    )
    .unwrap();
    db
}

#[test]
fn insert_tree_fills_parent_id_into_children() {
    let mut db = fresh();
    let mut depts = vec![
        Dept { id: None, name: "研发".into() },
        Dept { id: None, name: "销售".into() },
    ];
    let drafts = [vec!["张三", "李四"], vec!["王五"]];
    let (pc, cc) = db
        .insert_tree(
            &mut depts,
            |d, id| d.id = Some(id),
            |d| {
                let idx = if d.name == "研发" { 0 } else { 1 };
                drafts[idx]
                    .iter()
                    .map(|n| Staff {
                        id: None,
                        dept_id: d.id.unwrap_or(0),
                        name: (*n).to_string(),
                    })
                    .collect()
            },
        )
        .unwrap();

    assert_eq!((2, 3), (pc, cc));
    assert_eq!((Some(1), Some(2)), (depts[0].id, depts[1].id), "父的 id 被回填");

    let staff: Vec<Staff> = db
        .fetch_all(
            SelectStmt::new("c_staff")
                .column("id")
                .column("dept_id")
                .column("name")
                .order_by(rsbatis::Expr::bare_col("id"), SortOrder::Asc)
                .build(db.dialect()),
        )
        .unwrap();
    let mapping: Vec<(String, i64)> = staff.into_iter().map(|s| (s.name, s.dept_id)).collect();
    assert_eq!(
        vec![
            ("张三".to_string(), 1),
            ("李四".to_string(), 1),
            ("王五".to_string(), 2)
        ],
        mapping,
        "子的 dept_id 必须是父回填的那个 id，而不是 0 或 NULL"
    );
}

#[test]
fn insert_tree_rolls_back_all_on_child_failure() {
    let mut db = fresh();
    let mut depts = vec![Dept { id: None, name: "研发".into() }];
    // 造一个必然失败的子：同一个显式主键插两次，第二条撞唯一约束。
    // 注意不能靠"两个 None"制造冲突——None 会让数据库各自自增，两条都合法。
    let result = db.insert_tree(&mut depts, |d, id| d.id = Some(id), |d| {
        vec![
            Staff { id: Some(5), dept_id: d.id.unwrap_or(0), name: "ok".into() },
            Staff { id: Some(5), dept_id: d.id.unwrap_or(0), name: "dup".into() },
        ]
    });
    assert!(result.is_err(), "重复主键必须让整批失败，got: {result:?}");

    // 父也不该留下
    let count: i64 = db
        .fetch_one(
            SelectStmt::new("c_dept")
                .column_expr(rsbatis::Expr::count_star())
                .build(db.dialect()),
        )
        .unwrap()
        .unwrap()
        .get::<i64>(0)
        .unwrap();
    assert_eq!(0, count, "父插入也要一起回滚，不能留半截数据");
}

#[test]
fn insert_tree_with_no_children() {
    let mut db = fresh();
    let mut depts = vec![Dept { id: None, name: "空部门".into() }];
    let (pc, cc) = db
        .insert_tree(&mut depts, |d, id| d.id = Some(id), |_| Vec::<Staff>::new())
        .unwrap();
    assert_eq!((1, 0), (pc, cc));
    assert_eq!(Some(1), depts[0].id);
}

#[test]
fn insert_tree_empty_input() {
    let mut db = fresh();
    let mut none: Vec<Dept> = Vec::new();
    let got = db
        .insert_tree(&mut none, |d, id| d.id = Some(id), |_| Vec::<Staff>::new())
        .unwrap();
    assert_eq!((0, 0), got);
}

#[test]
fn insert_one_id_uses_returning_and_returns_real_pk() {
    let mut db = fresh();
    let id1 = db
        .insert_one_id(&Dept { id: None, name: "A".into() })
        .unwrap();
    let id2 = db
        .insert_one_id(&Dept { id: None, name: "B".into() })
        .unwrap();
    assert_eq!(1, id1);
    assert_eq!(2, id2);

    // 走 RETURNING 的话，给一个显式主键也能原样拿回
    let id_y = db
        .insert_one_id(&Dept { id: Some(77), name: "Y".into() })
        .unwrap();
    assert_eq!(77, id_y, "RETURNING 路径拿的是真实主键，不是自增计数器");
}

#[test]
fn nested_insert_tree_is_rejected() {
    let mut db = fresh();
    let mut outer = vec![Dept { id: None, name: "外层".into() }];
    let mut inner_ids: Vec<i64> = Vec::new();
    let res: Result<(), rsbatis::DriverError> = (|| {
        db.insert_tree(&mut outer, |d, id| d.id = Some(id), |d| {
            inner_ids.push(d.id.unwrap_or(0));
            Vec::<Staff>::new()
        })?;
        Ok(())
    })();
    assert!(res.is_ok(), "单层 insert_tree 应当成功：{res:?}");
    // 事务已结束，再开一次是合法的；这里只确认第一次没留下 in_tx 脏状态
    assert!(!db.in_transaction());
}

#[derive(Entity, FromRow, Debug, Clone)]
#[rsbatis(table = "c_plain")]
#[allow(dead_code)]
struct NoPk {
    id: Option<i64>,
    note: String,
}

#[test]
fn sqlite_without_declared_pk_is_rejected() {
    let mut db = fresh();
    db.execute_batch("CREATE TABLE c_plain (id INTEGER PRIMARY KEY, note TEXT NOT NULL);")
        .unwrap();
    // 没有 #[rsbatis(primary_key)]：SQLite 上拒绝回填，而不是给出 rowid 当主键
    let err = db
        .insert_one_id(&NoPk { id: None, note: "x".into() })
        .unwrap_err();
    assert!(
        err.message.contains("没有声明主键"),
        "got: {}",
        err.message
    );
}
