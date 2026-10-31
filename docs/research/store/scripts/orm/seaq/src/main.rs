use sea_query::{Alias, Expr, Func, Query, SqliteQueryBuilder};
use sea_query_rusqlite::RusqliteBinder;

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let conn = rusqlite::Connection::open(path).unwrap();
    let (sql, values) = Query::select()
        .expr(Func::count(Expr::col(Alias::new("name"))))
        .from(Alias::new("sqlite_schema"))
        .build_rusqlite(SqliteQueryBuilder);
    let n: i64 = conn
        .query_row(&sql, &*values.as_params(), |row| row.get(0))
        .unwrap();
    println!("{n}");
}
