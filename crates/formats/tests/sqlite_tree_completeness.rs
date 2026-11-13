use craft_anything_formats::sqlite::{CellValue, SqliteDb};
use craft_anything_testkit::sqlite::{PAGE_SIZE as PAGE, Val, table_db};

fn tree(left: u32, right: u32) -> Vec<u8> {
    let mut data = table_db("CREATE TABLE t(value TEXT)", &[(1, vec![Val::Text("kept".into())])]);
    let leaf = data[PAGE..2 * PAGE].to_vec();
    data.resize(4 * PAGE, 0);
    data[28..32].copy_from_slice(&4u32.to_be_bytes());
    data[2 * PAGE..3 * PAGE].copy_from_slice(&leaf);
    data[3 * PAGE..4 * PAGE].copy_from_slice(&leaf);
    let cell = u16::from_be_bytes([leaf[8], leaf[9]]) as usize;
    data[3 * PAGE + cell + 1] = 0; // Left rowid 0, right rowid 1.
    data[PAGE..2 * PAGE].fill(0);
    data[PAGE] = 0x05;
    data[PAGE + 3..PAGE + 5].copy_from_slice(&1u16.to_be_bytes());
    data[PAGE + 5..PAGE + 7].copy_from_slice(&((PAGE - 5) as u16).to_be_bytes());
    data[PAGE + 8..PAGE + 12].copy_from_slice(&right.to_be_bytes());
    data[PAGE + 12..PAGE + 14].copy_from_slice(&((PAGE - 5) as u16).to_be_bytes());
    data[2 * PAGE - 5..2 * PAGE - 1].copy_from_slice(&left.to_be_bytes());
    data[2 * PAGE - 1] = 0; // The largest rowid in the left subtree.
    data
}

fn assert_partial(data: &[u8], rows: usize) {
    let db = SqliteDb::parse(data).unwrap();
    let count = db.count_rows(2);
    assert_eq!(count.count, rows as u64);
    assert!(count.bounded, "a damaged subtree must make the count a lower bound");
    let page = db.read_rows(2, 0, 10);
    assert_eq!(page.rows.len(), rows);
    assert!(page.truncated, "a damaged subtree may hide rows below the requested limit");
    assert!(!page.notes.is_empty(), "the skipped subtree must be disclosed");
    for row in page.rows {
        assert_eq!(row.values, vec![CellValue::Text { text: "kept".into(), truncated: false }]);
    }
}

#[test]
fn ancestor_cycles_leave_the_other_child_readable_but_incomplete() {
    assert_partial(&tree(2, 3), 1);
    assert_partial(&tree(4, 2), 1);
}

#[test]
fn null_children_leave_the_other_child_readable_but_incomplete() {
    assert_partial(&tree(0, 3), 1);
    assert_partial(&tree(4, 0), 1);
}

#[test]
fn a_reused_leaf_is_counted_once_and_disclosed() {
    assert_partial(&tree(3, 3), 1);
}

#[test]
fn repeated_bad_children_do_not_repeat_the_same_note() {
    let data = tree(0, 0);
    let page = SqliteDb::parse(&data).unwrap().read_rows(2, 0, 10);
    assert!(page.truncated);
    assert_eq!(page.notes.len(), 1);
}

#[test]
fn missing_and_wrong_kind_children_are_incomplete() {
    assert_partial(&tree(5, 3), 1);
    let mut data = tree(4, 3);
    data[3 * PAGE] = 0x0a; // An index leaf cannot be a table b-tree child.
    assert_partial(&data, 1);
}

#[test]
fn without_rowid_trees_disclose_null_and_cyclic_children() {
    use craft_anything_testkit::sqlite::without_rowid_db;

    for child in [0u32, 2] {
        let mut data = without_rowid_db("CREATE TABLE t(value TEXT PRIMARY KEY) WITHOUT ROWID", &[]);
        data[PAGE..2 * PAGE].fill(0);
        data[PAGE] = 0x02;
        data[PAGE + 5..PAGE + 7].copy_from_slice(&(PAGE as u16).to_be_bytes());
        data[PAGE + 8..PAGE + 12].copy_from_slice(&child.to_be_bytes());
        assert_partial(&data, 0);
    }
}

#[test]
fn a_depth_limit_is_reported_even_below_the_row_limit() {
    let mut data = tree(4, 3);
    let pages = 70usize;
    data.resize(pages * PAGE, 0);
    data[28..32].copy_from_slice(&(pages as u32).to_be_bytes());
    for page in 2..pages {
        let start = (page - 1) * PAGE;
        data[start..start + PAGE].fill(0);
        data[start] = 0x05;
        data[start + 5..start + 7].copy_from_slice(&(PAGE as u16).to_be_bytes());
        data[start + 8..start + 12].copy_from_slice(&((page + 1) as u32).to_be_bytes());
    }
    assert_partial(&data, 0);
}

#[test]
fn complete_trees_keep_counts_order_and_pagination() {
    let data = tree(4, 3);
    let db = SqliteDb::parse(&data).unwrap();
    let count = db.count_rows(2);
    assert_eq!(count.count, 2);
    assert!(!count.bounded);
    let page = db.read_rows(2, 0, 10);
    assert_eq!(page.rows.iter().map(|r| r.rowid).collect::<Vec<_>>(), vec![Some(0), Some(1)]);
    assert!(!page.truncated);
    assert!(page.notes.is_empty());
    let tail = db.read_rows(2, 1, 10);
    assert_eq!(tail.offset, 1);
    assert_eq!(tail.rows.len(), 1);
    assert_eq!(tail.rows[0].rowid, Some(1));
    assert!(!tail.truncated);
    assert!(tail.notes.is_empty());
    let capped = db.read_rows(2, 0, 1);
    assert_eq!(capped.rows.len(), 1);
    assert!(capped.truncated);
    assert!(capped.notes.is_empty());
}

#[test]
fn an_empty_leaf_is_complete() {
    let data = table_db("CREATE TABLE t(value TEXT)", &[]);
    let db = SqliteDb::parse(&data).unwrap();
    let count = db.count_rows(2);
    assert_eq!(count.count, 0);
    assert!(!count.bounded);
    let page = db.read_rows(2, 0, 10);
    assert!(page.rows.is_empty());
    assert!(!page.truncated);
    assert!(page.notes.is_empty());
}
