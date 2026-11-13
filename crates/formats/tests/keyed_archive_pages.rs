use craft_anything_formats::keyed_archive;
use craft_anything_testkit::apple as fx;

#[test]
fn pages_preserve_row_contents_order_and_absolute_indices() {
    let archive = keyed_archive::parse(&fx::keyed_archive()).unwrap();
    let all = archive.rows();
    for (offset, limit) in [(0, 0), (0, 1), (1, 3), (all.len() - 1, usize::MAX), (all.len(), 3), (all.len() + 1, 3), (usize::MAX, usize::MAX)] {
        let expected: Vec<_> = all.iter().skip(offset).take(limit).collect();
        let actual = archive.rows_page(offset, limit);
        assert_eq!(serde_json::to_value(actual).unwrap(), serde_json::to_value(expected).unwrap(), "offset {offset}, limit {limit}");
    }
}

#[test]
fn class_names_borrow_the_shared_object() {
    let archive = keyed_archive::parse(&fx::keyed_archive()).unwrap();
    let root = archive.root(None).unwrap().1;
    let class_index = archive.object(root).unwrap().get("$class").and_then(keyed_archive::uid_of).unwrap();
    let stored = archive.object(class_index).unwrap().get("$classname").unwrap().as_str().unwrap();
    let name = archive.class_name(root).unwrap();
    assert!(std::ptr::eq(name.as_ptr(), stored.as_ptr()));
    assert_eq!(archive.class_of(root).as_deref(), Some(name));
    assert!(archive.class_name(u64::MAX).is_none());
}

#[test]
fn a_small_page_copies_only_its_requested_large_class_name() {
    let class = "C".repeat(64 << 10);
    let mut objects = vec![fx::s("$null"), fx::dict(vec![("$classname", fx::s(&class))])];
    objects.extend((0..1024).map(|_| fx::dict(vec![("$class", fx::P::Uid(1))])));
    let bytes = fx::to_bplist(&fx::dict(vec![("$objects", fx::P::Array(objects)), ("$top", fx::dict(vec![("root", fx::P::Uid(2))]))]));
    let archive = keyed_archive::parse(&bytes).unwrap();
    let page = archive.rows_page(513, 1);
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].index, 513);
    assert_eq!(page[0].class.as_deref(), Some(class.as_str()));
    assert_eq!(page[0].references, [1]);
    assert_eq!(page.iter().filter_map(|row| row.class.as_ref()).map(String::len).sum::<usize>(), 64 << 10);
}
