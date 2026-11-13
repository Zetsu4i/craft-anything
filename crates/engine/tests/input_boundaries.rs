use craft_anything_engine::Target;
use craft_anything_engine::evidence::ErrorCode;
use craft_anything_engine::tools::static_input;
#[cfg(unix)]
use craft_anything_engine::{Caller, CancelToken, Ctx};

#[cfg(target_os = "linux")]
#[test]
fn text_tool_refuses_a_pipe_without_waiting_for_its_writer() {
    use craft_anything_engine::{CallOptions, Engine};
    use std::os::fd::AsRawFd;
    use std::sync::mpsc;
    use std::time::Duration;

    let (reader, writer) = std::io::pipe().unwrap();
    let path = format!("/proc/self/fd/{}", reader.as_raw_fd());
    let (sender, receiver) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let engine = Engine::new(craft_anything_engine::testing::session());
        let result = engine.call("core_text_read", serde_json::json!({"path": path}), CallOptions::new(Caller::Cli));
        drop(reader);
        sender.send(result).ok();
    });
    let immediate = receiver.recv_timeout(Duration::from_secs(1));
    // Release a baseline read waiting for EOF before checking the bounded response.
    drop(writer);
    if immediate.is_err() {
        let _ = receiver.recv_timeout(Duration::from_secs(1));
    }
    if worker.is_finished() {
        worker.join().unwrap();
    }
    let result = immediate.expect("reading a pipe waited for its writer instead of rejecting it");
    assert_eq!(result.unwrap_err().code, ErrorCode::BadInput);
}

#[cfg(target_os = "linux")]
#[test]
fn load_path_checks_actual_bytes_when_metadata_reports_zero() {
    let path = "/proc/self/stat";
    assert_eq!(std::fs::metadata(path).unwrap().len(), 0);
    let result = static_input::load_path(path, 1);
    assert!(result.is_err(), "a one-byte cap accepted {} bytes", result.as_ref().map_or(0, |input| input.bytes.len()));
    assert_eq!(result.unwrap_err().code, ErrorCode::OverLimit);
}

#[cfg(target_os = "linux")]
#[test]
fn prefix_reader_checks_eof_before_hashing_zero_size_metadata() {
    let mut session = craft_anything_engine::testing::session();
    let ctx = Ctx { tool: "test", session: &mut session, cancel: CancelToken::default(), progress: &|_| {}, caller: Caller::Cli };
    for cap in [1, 0] {
        let prefix = static_input::load_prefix(&ctx, Some("/proc/self/stat"), cap).unwrap();
        assert_eq!(prefix.bytes.len(), cap as usize);
        assert_eq!(prefix.file_size, 0, "keep the metadata size rather than guessing a full length");
        assert!(!prefix.whole, "a bounded prefix was reported as the whole virtual file");
        assert!(prefix.subject.sha256.is_none(), "a prefix hash must not identify the whole file");
    }
    let whole = static_input::load_prefix(&ctx, Some("/proc/self/stat"), 16 * 1024).unwrap();
    assert!(!whole.bytes.is_empty());
    assert!(whole.whole);
    assert_eq!(whole.subject.sha256, Some(craft_anything_engine::evidence::sha256_hex(&whole.bytes)));
}

#[cfg(target_os = "linux")]
#[test]
fn archive_input_checks_actual_bytes_when_metadata_reports_zero() {
    use craft_anything_engine::{CallOptions, Engine};
    let engine = Engine::new(craft_anything_engine::testing::session());
    let result = engine.call("core_archive_show", serde_json::json!({"path": "/proc/self/stat", "max_total_bytes": 1}), CallOptions::new(Caller::Cli));
    assert!(result.is_err(), "archive input read beyond its one-byte cap");
    assert_eq!(result.unwrap_err().code, ErrorCode::OverLimit);
}

#[cfg(unix)]
#[test]
fn load_path_refuses_a_non_regular_file() {
    let result = static_input::load_path("/dev/null", 16);
    assert!(result.is_err(), "a character device was accepted as a file");
    assert_eq!(result.unwrap_err().code, ErrorCode::BadInput);
}

#[cfg(unix)]
#[test]
fn prefix_reader_refuses_a_non_regular_file() {
    let mut session = craft_anything_engine::testing::session();
    let ctx = Ctx { tool: "test", session: &mut session, cancel: CancelToken::default(), progress: &|_| {}, caller: Caller::Cli };
    let result = static_input::load_prefix(&ctx, Some("/dev/null"), 16);
    assert!(result.is_err(), "a character device was accepted as a prefix input");
    assert_eq!(result.unwrap_err().code, ErrorCode::BadInput);
}

#[cfg(unix)]
#[test]
fn targets_refuse_a_non_regular_file() {
    let result = Target::load(std::path::Path::new("/dev/null"));
    assert!(result.is_err(), "a character device was accepted as a target");
    assert_eq!(result.unwrap_err().code, ErrorCode::BadInput);
}

#[test]
fn ordinary_files_keep_exact_cap_and_empty_input_support() {
    let directory = craft_anything_engine::testing::temp_dir();
    let path = directory.join("input.bin");
    std::fs::write(&path, b"four").unwrap();
    let path = path.to_str().unwrap();
    assert_eq!(&*static_input::load_path(path, 4).unwrap().bytes, b"four");
    assert_eq!(&*static_input::load_path(path, u64::MAX).unwrap().bytes, b"four");
    assert_eq!(static_input::load_path(path, 3).unwrap_err().code, ErrorCode::OverLimit);
    std::fs::write(path, b"").unwrap();
    assert!(static_input::load_path(path, 0).unwrap().bytes.is_empty());
    assert!(Target::load(&directory).unwrap().bytes.is_empty());
    std::fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn regular_file_symlinks_and_prefix_reads_keep_working() {
    let directory = craft_anything_engine::testing::temp_dir();
    let path = directory.join("file.bin");
    let link = directory.join("link.bin");
    std::fs::write(&path, b"hello").unwrap();
    std::os::unix::fs::symlink(&path, &link).unwrap();
    let link_name = link.to_str().unwrap();
    assert_eq!(&*static_input::load_path(link_name, 5).unwrap().bytes, b"hello");
    assert_eq!(&*Target::load(&link).unwrap().bytes, b"hello");
    let mut session = craft_anything_engine::testing::session();
    let ctx = Ctx { tool: "test", session: &mut session, cancel: CancelToken::default(), progress: &|_| {}, caller: Caller::Cli };
    let prefix = static_input::load_prefix(&ctx, Some(link_name), 3).unwrap();
    assert_eq!(prefix.bytes, b"hel");
    assert_eq!(prefix.file_size, 5);
    assert!(!prefix.whole);
    assert!(prefix.subject.sha256.is_none());
    let whole = static_input::load_prefix(&ctx, Some(link_name), 5).unwrap();
    assert_eq!(whole.bytes, b"hello");
    assert!(whole.whole);
    assert!(whole.subject.sha256.is_some());
    std::fs::remove_dir_all(directory).unwrap();
}
