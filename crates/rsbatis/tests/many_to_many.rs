
//! 多对多预加载：`关联表 IN` → `子表 IN`。
//!
//! 重点验证三种"数据不干净"的情况都不 panic、不静默给错类型的结果。

#![cfg(feature = "sqlite")]

use rsbatis::{Entity, FromRow, HasMany, ManyToMany, Session, SqliteDriver};

#[derive(Entity, FromRow, Clone, Debug, PartialEq)]
#[rsbatis(table = "m2m_student")]
#[allow(dead_code)]
struct Student {
    id: i64,
    name: String,
}

#[derive(Entity, FromRow, Clone, Debug, PartialEq)]
#[rsbatis(table = "m2m_course")]
#[allow(dead_code)]
struct Course {
    id: i64,
    title: String,
}

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "m2m_enroll")]
#[allow(dead_code)]
struct Enroll {
    student_id: i64,
    course_id: i64,
}

fn seeded() -> Session<SqliteDriver> {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    db.execute_batch(
        "CREATE TABLE m2m_student (id INTEGER NOT NULL, name TEXT NOT NULL);
         CREATE TABLE m2m_course (id INTEGER NOT NULL, title TEXT NOT NULL);
         CREATE TABLE m2m_enroll (student_id INTEGER NOT NULL, course_id INTEGER NOT NULL);
         INSERT INTO m2m_student (id, name) VALUES (1, 'A'), (2, 'B'), (3, 'C');
         INSERT INTO m2m_course (id, title) VALUES (10, 'Rust'), (11, 'SQL'), (12, 'Go');
         -- A 选 Rust+SQL，B 选 SQL，C 什么都没选
         INSERT INTO m2m_enroll (student_id, course_id) VALUES (1, 10), (1, 11), (2, 11);",
    )
    .unwrap();
    db
}

fn rel() -> ManyToMany<Course> {
    // 前两个参数是**关联表**的列（student_id / course_id），第三个才是子表的列。
    // 传成 `Student::col_id()` 会渲染出 `WHERE "id"` 去查没有 id 列的关联表 →
    // 静默查出空集合，所以 load 时会校验表名归属。
    ManyToMany::new(
        "m2m_enroll",
        Enroll::col_student_id(),
        Enroll::col_course_id(),
        Course::col_id(),
        |c| c.id,
    )
}

fn students(db: &mut Session<SqliteDriver>) -> Vec<Student> {
    db.fetch_all(
        rsbatis::SelectStmt::new("m2m_student")
            .column("id")
            .column("name")
            .order_by(rsbatis::Expr::bare_col("id"), rsbatis::SortOrder::Asc)
            .build(db.dialect()),
    )
    .unwrap()
}

#[test]
fn links_are_bucketed_per_parent_in_input_order() {
    let mut db = seeded();
    let ss = students(&mut db);
    let got = db.load_many_to_many(&rel(), &ss, |s| s.id).unwrap();

    assert_eq!(3, got.len());
    assert_eq!(vec![10, 11], got[0].iter().map(|c| c.id).collect::<Vec<_>>());
    assert_eq!(vec![11], got[1].iter().map(|c| c.id).collect::<Vec<_>>());
    assert!(got[2].is_empty(), "C 没选课，应该是空集合而不是缺一项");
}

#[test]
fn titles_are_correctly_joined() {
    let mut db = seeded();
    let ss = students(&mut db);
    let got = db.load_many_to_many(&rel(), &ss, |s| s.id).unwrap();
    assert_eq!("Rust", got[0][0].title);
    assert_eq!("SQL", got[0][1].title);
    assert_eq!("SQL", got[1][0].title);
}

#[test]
fn empty_parent_set_makes_no_query() {
    let mut db = seeded();
    let got = db.load_many_to_many(&rel(), &[] as &[Student], |s| s.id).unwrap();
    assert!(got.is_empty());
}

#[test]
fn duplicate_parent_input_still_gets_children() {
    let mut db = seeded();
    let ss = students(&mut db);
    // 同一个父传两次：两次都必须拿到课程，不能用 remove 让第二次变空
    let twice = vec![ss[0].clone(), ss[0].clone()];
    let got = db.load_many_to_many(&rel(), &twice, |s| s.id).unwrap();
    assert_eq!(2, got.len());
    assert_eq!(2, got[0].len());
    assert_eq!(2, got[1].len(), "重复输入的那一份不该变成空集合");
}

#[test]
fn parent_without_links_gets_empty_vec_not_skipped() {
    let mut db = seeded();
    let ss = students(&mut db);
    let got = db.load_many_to_many(&rel(), &ss, |s| s.id).unwrap();
    assert_eq!(3, got.len(), "没有关联的父也要占一个位置");
}

#[test]
fn orphan_link_to_deleted_child_is_skipped() {
    let mut db = seeded();
    // 加一条指向不存在的课程 99 的关联
    db.execute_batch("INSERT INTO m2m_enroll (student_id, course_id) VALUES (1, 99);")
        .unwrap();
    let ss = students(&mut db);
    let got = db.load_many_to_many(&rel(), &ss, |s| s.id).unwrap();
    assert_eq!(
        vec![10, 11],
        got[0].iter().map(|c| c.id).collect::<Vec<_>>(),
        "指向已删除课程的关联要被跳过，而不是塞个占位或 panic"
    );
}

#[test]
fn duplicate_link_yields_duplicate_child_by_design() {
    let mut db = seeded();
    // 同一对 (1, 10) 插两次 —— 关联表没有唯一约束时的真实脏数据
    db.execute_batch("INSERT INTO m2m_enroll (student_id, course_id) VALUES (1, 10);")
        .unwrap();
    let ss = students(&mut db);
    let got = db.load_many_to_many(&rel(), &ss, |s| s.id).unwrap();
    assert_eq!(
        vec![10, 11, 10],
        got[0].iter().map(|c| c.id).collect::<Vec<_>>(),
        "ORM 不去重：悄悄去重会掩盖脏数据"
    );
}

#[test]
fn reverse_direction_uses_same_descriptor_shape() {
    let mut db = seeded();
    // 课程 → 学生：把关联表的两列对调即可，同一个 API 不用改
    let rev = ManyToMany::<Student>::new(
        "m2m_enroll",
        Enroll::col_course_id(),
        Enroll::col_student_id(),
        Student::col_id(),
        |s| s.id,
    );
    let courses: Vec<Course> = db
        .fetch_all(
            rsbatis::SelectStmt::new("m2m_course")
                .column("id")
                .column("title")
                .build(db.dialect()),
        )
        .unwrap();
    let got = db.load_many_to_many(&rev, &courses, |c| c.id).unwrap();
    let mut counts: Vec<usize> = got.iter().map(|v| v.len()).collect();
    counts.sort();
    assert_eq!(vec![0, 1, 2], counts, "Rust 1 人、SQL 2 人、Go 0 人");
}

#[test]
fn has_many_still_works_alongside() {
    // 同一张子表用 HasMany（子表里有父的列）走另一条路径，确认两个描述符不冲突
    let mut db = seeded();
    let ss = students(&mut db);
    let hm = HasMany::<Enroll>::new(Enroll::col_student_id(), |e| e.student_id);
    let got = db.load_has_many(&hm, &ss, |s| s.id).unwrap();
    assert_eq!(vec![2, 1, 0], got.iter().map(|v| v.len()).collect::<Vec<_>>());
}

#[test]
fn wrong_link_column_is_rejected_instead_of_returning_empty() {
    let mut db = seeded();
    let ss = students(&mut db);
    // 故意把父表的主键当关联表的列传
    let bad = ManyToMany::<Course>::new(
        "m2m_enroll",
        Student::col_id(),
        Enroll::col_course_id(),
        Course::col_id(),
        |c| c.id,
    );
    let err = db.load_many_to_many(&bad, &ss, |s| s.id).unwrap_err();
    assert!(
        err.message.contains("link_parent") && err.message.contains("m2m_enroll"),
        "错要指出是哪一列、应该是哪张表，got: {}",
        err.message
    );
}

#[test]
fn wrong_child_column_is_rejected() {
    let mut db = seeded();
    let ss = students(&mut db);
    // child_col 给了关联表的列
    let bad = ManyToMany::<Course>::new(
        "m2m_enroll",
        Enroll::col_student_id(),
        Enroll::col_course_id(),
        Enroll::col_course_id(),
        |c| c.id,
    );
    let err = db.load_many_to_many(&bad, &ss, |s| s.id).unwrap_err();
    assert!(err.message.contains("child_col"), "got: {}", err.message);
}
