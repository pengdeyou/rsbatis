
//! 多对多预加载：学生 ↔ 课程，靠 `enroll` 关联表连着。
//!
//! 运行：`cargo run -p rsbatis --features sqlite --example many_to_many`

use rsbatis::{Entity, FromRow, ManyToMany, SelectStmt, Session, SqliteDriver, SortOrder};

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "student")]
#[allow(dead_code)]
struct Student {
    id: i64,
    name: String,
}

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "course")]
#[allow(dead_code)]
struct Course {
    id: i64,
    title: String,
}

#[derive(Entity, FromRow, Clone, Debug)]
#[rsbatis(table = "enroll")]
#[allow(dead_code)]
struct Enroll {
    student_id: i64,
    course_id: i64,
}

fn main() {
    let mut db = Session::new(SqliteDriver::open_in_memory().unwrap());
    db.execute_batch(
        "CREATE TABLE student (id INTEGER NOT NULL, name TEXT NOT NULL);
         CREATE TABLE course (id INTEGER NOT NULL, title TEXT NOT NULL);
         CREATE TABLE enroll (student_id INTEGER NOT NULL, course_id INTEGER NOT NULL);
         INSERT INTO student (id, name) VALUES (1, '小明'), (2, '小红'), (3, '小刚');
         INSERT INTO course (id, title) VALUES (10, 'Rust'), (11, '数据库');
         INSERT INTO enroll (student_id, course_id) VALUES (1, 10), (1, 11), (2, 11);",
    )
    .unwrap();

    let rel = ManyToMany::<Course>::new(
        "enroll",
        Enroll::student_id(),
        Enroll::course_id(),
        Course::id(),
        |c| c.id,
    );

    let students: Vec<Student> = db
        .fetch_all(
            SelectStmt::new("student")
                .column("id")
                .column("name")
                .order_by(rsbatis::Expr::bare_col("id"), SortOrder::Asc)
                .build(db.dialect()),
        )
        .unwrap();

    let courses = db.load_many_to_many(&rel, &students, |s| s.id).unwrap();
    for (s, cs) in students.iter().zip(courses.iter()) {
        let titles: Vec<&str> = cs.iter().map(|c| c.title.as_str()).collect();
        println!("{} 选了 {:?}", s.name, titles);
    }

    // 反向：一门课被哪些人选了 —— 把关联表的两列对调，同一个 API
    let rev = ManyToMany::<Student>::new(
        "enroll",
        Enroll::course_id(),
        Enroll::student_id(),
        Student::id(),
        |s| s.id,
    );
    let courses: Vec<Course> = db
        .fetch_all(
            SelectStmt::new("course")
                .column("id")
                .column("title")
                .order_by(rsbatis::Expr::bare_col("id"), SortOrder::Asc)
                .build(db.dialect()),
        )
        .unwrap();
    let enrolled = db.load_many_to_many(&rev, &courses, |c| c.id).unwrap();
    for (c, ss) in courses.iter().zip(enrolled.iter()) {
        let names: Vec<&str> = ss.iter().map(|s| s.name.as_str()).collect();
        println!("{} 被 {:?} 选", c.title, names);
    }

    println!("\n两次往返拿下全场：1 次关联表 IN + 1 次子表 IN，与父/子数量无关");
}
