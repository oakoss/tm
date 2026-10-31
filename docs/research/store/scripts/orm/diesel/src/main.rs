use diesel::prelude::*;

#[derive(QueryableByName)]
struct Count {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    n: i64,
}

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let mut conn = SqliteConnection::establish(&path).unwrap();
    let count: Count = diesel::sql_query("SELECT count(*) AS n FROM sqlite_schema")
        .get_result(&mut conn)
        .unwrap();
    println!("{}", count.n);
}
