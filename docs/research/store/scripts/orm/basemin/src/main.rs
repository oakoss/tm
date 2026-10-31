fn main() {
    let path = std::env::args().nth(1).unwrap();
    let conn = rusqlite::Connection::open(path).unwrap();
    let n: i64 = conn
        .query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))
        .unwrap();
    println!("{n}");
}
