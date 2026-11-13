use craft_anything_formats::sqlite::{CellValue, SqliteDb};
use craft_anything_testkit::sqlite::{PAGE_SIZE, Val, table_db, without_rowid_db};

fn text(value: &str) -> CellValue {
    CellValue::Text { text: value.into(), truncated: false }
}

/// Skipping rows must preserve rowid aliases, defaults, end offsets and empty-page semantics.
#[test]
fn rowid_pages_keep_only_the_requested_rows_in_declaration_order() {
    let rows: Vec<_> = (1..=7).map(|i| (i, vec![Val::Null, Val::Text(format!("row {i}"))])).collect();
    let bytes = table_db("CREATE TABLE t(id INTEGER PRIMARY KEY, value TEXT, extra INT DEFAULT 42)", &rows);
    let db = SqliteDb::parse(&bytes).unwrap();
    for (offset, limit) in [(0, 1), (1, 2), (5, 2), (6, 5), (7, 1), (99, 5), (usize::MAX, 1), (0, 0), (3, 0), (99, 0)] {
        let page = db.read_rows(2, offset, limit);
        assert_eq!(page.offset, offset.min(rows.len()));
        let expected: Vec<_> = rows.iter().skip(offset).take(limit).map(|(id, _)| *id).collect();
        assert_eq!(page.rows.iter().map(|row| row.rowid.unwrap()).collect::<Vec<_>>(), expected);
        for row in &page.rows {
            let id = row.rowid.unwrap();
            assert_eq!(row.values, vec![CellValue::Int(id), text(&format!("row {id}")), CellValue::Int(42)]);
        }
        assert_eq!(page.truncated, offset.saturating_add(limit) > 0 && offset.saturating_add(limit) <= rows.len());
        assert!(page.notes.is_empty(), "{:?}", page.notes);
    }
}

/// Index b-trees store rows in both leaves and interior pages, and primary keys may be reordered.
#[test]
fn without_rowid_pages_skip_interior_records_and_restore_column_order() {
    let rows: Vec<_> = (0..40).map(|i| vec![Val::Text(format!("key {i:02}")), Val::Int(i)]).collect();
    let bytes = without_rowid_db("CREATE TABLE t(value INT, key TEXT PRIMARY KEY, extra INT DEFAULT 9) WITHOUT ROWID", &rows);
    let db = SqliteDb::parse(&bytes).unwrap();
    for (offset, limit) in [(0, 2), (2, 7), (18, 5), (37, 9), (40, 1), (usize::MAX, 1), (0, 0), (18, 0), (99, 0)] {
        let page = db.read_rows(2, offset, limit);
        assert_eq!(page.offset, offset.min(rows.len()));
        assert_eq!(page.rows.len(), rows.len().saturating_sub(offset).min(limit));
        for (i, row) in page.rows.iter().enumerate() {
            let value = offset + i;
            assert_eq!(row.rowid, None);
            assert_eq!(row.values, vec![CellValue::Int(value as i64), text(&format!("key {value:02}")), CellValue::Int(9)]);
        }
        assert!(page.notes.is_empty(), "{:?}", page.notes);
    }
}

/// The offset counts decoded rows, not unreadable cells; their damage remains disclosed.
#[test]
fn unreadable_rows_do_not_consume_the_requested_offset() {
    let rows: Vec<_> = (1..=4).map(|i| (i, vec![Val::Int(i)])).collect();
    let mut bytes = table_db("CREATE TABLE t(value INT)", &rows);
    bytes[PAGE_SIZE + 8..PAGE_SIZE + 10].copy_from_slice(&8u16.to_be_bytes());
    let db = SqliteDb::parse(&bytes).unwrap();
    let page = db.read_rows(2, 1, 1);
    assert_eq!(page.offset, 1);
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.rows[0].rowid, Some(3));
    assert!(page.notes.iter().any(|note| note.contains("unreadable row")));
    let past_end = db.read_rows(2, usize::MAX, 1);
    assert_eq!(past_end.offset, 3);
    assert!(past_end.rows.is_empty());
    assert!(past_end.notes.iter().any(|note| note.contains("unreadable row")));
}

/// A zero-sized first page requests no table records and must not start an unlimited walk.
#[test]
fn an_empty_first_page_does_not_read_table_cells() {
    let mut bytes = table_db("CREATE TABLE t(value INT)", &[(1, vec![Val::Int(1)])]);
    bytes[PAGE_SIZE + 8..PAGE_SIZE + 10].copy_from_slice(&8u16.to_be_bytes());
    let page = SqliteDb::parse(&bytes).unwrap().read_rows(2, 0, 0);
    assert_eq!(page.offset, 0);
    assert!(page.rows.is_empty());
    assert!(!page.truncated);
    assert!(page.notes.is_empty(), "no table cells were requested: {:?}", page.notes);
}
