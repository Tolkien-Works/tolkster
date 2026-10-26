use super::*;

fn dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tolkster-disk-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

#[test]
fn a_new_run_appends_to_the_lines_the_run_before_left() {
    let path = dir("append").join("nested").join("listen.log");
    Log::open(path.clone(), 1000).unwrap().keep("first run").unwrap();
    Log::open(path.clone(), 1000).unwrap().keep("second run").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "first run\nsecond run\n");
    let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
}

#[test]
fn a_full_file_becomes_the_older_one_and_the_one_before_it_is_gone() {
    let dir = dir("rotate");
    let path = dir.join("listen.log");
    let mut log = Log::open(path.clone(), 20).unwrap();
    for line in ["aaaaaaaaa", "bbbbbbbbb", "ccccccccc", "ddddddddd", "eeeeeeeee"] {
        log.keep(line).unwrap();
    }
    assert_eq!(fs::read_to_string(&path).unwrap(), "eeeeeeeee\n");
    assert_eq!(fs::read_to_string(dir.join("listen.log.1")).unwrap(), "ccccccccc\nddddddddd\n");
    assert!(!dir.join("listen.log.2").exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_line_is_one_line_whatever_it_names() {
    let path = dir("one-line").join("listen.log");
    Log::open(path.clone(), 1000).unwrap().keep("TW-1 kept a\nfake line").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "TW-1 kept a fake line\n");
    let _ = fs::remove_dir_all(path.parent().unwrap());
}
