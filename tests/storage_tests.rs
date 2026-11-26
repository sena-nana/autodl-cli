use assert_cmd::Command;
use predicates::prelude::*;

use autodl_cli::ssh::storage::{
    AUTODL_FS_ROOT, DuEntry, HiddenCacheEntry, ensure_not_fs_root, parse_cache_list_output,
    parse_df_bytes_output, parse_df_inodes_output, parse_du_output, resolve_fs_path,
};

#[test]
fn test_resolve_fs_path_and_traversal_guard() {
    assert_eq!(resolve_fs_path(None).unwrap(), AUTODL_FS_ROOT);
    assert_eq!(resolve_fs_path(Some("")).unwrap(), AUTODL_FS_ROOT);
    assert_eq!(resolve_fs_path(Some(".")).unwrap(), AUTODL_FS_ROOT);
    assert_eq!(resolve_fs_path(Some("/")).unwrap(), AUTODL_FS_ROOT);
    assert_eq!(
        resolve_fs_path(Some("datasets/imagenet")).unwrap(),
        "/root/autodl-fs/datasets/imagenet"
    );
    assert_eq!(
        resolve_fs_path(Some("/root/autodl-fs/models/ckpt.pt")).unwrap(),
        "/root/autodl-fs/models/ckpt.pt"
    );
    assert_eq!(
        resolve_fs_path(Some("a/b/../c")).unwrap(),
        "/root/autodl-fs/a/c"
    );

    assert!(resolve_fs_path(Some("../../etc/passwd")).is_err());
    assert!(resolve_fs_path(Some("/root/autodl-fs/../../etc/shadow")).is_err());

    assert!(ensure_not_fs_root("/root/autodl-fs").is_err());
    assert!(ensure_not_fs_root("/root/autodl-fs/").is_err());
    assert!(ensure_not_fs_root("/root/autodl-fs/tmp.bin").is_ok());
}

#[test]
fn test_df_du_and_cache_parsers() {
    let df_bytes = "\
Filesystem     1B-blocks        Used   Available Capacity Mounted on
10.0.0.1:/fs  214748364800 10737418240 204010946560       5% /root/autodl-fs
";
    let (fs, total, used, avail, pct, mnt) = parse_df_bytes_output(df_bytes).unwrap();
    assert_eq!(fs, "10.0.0.1:/fs");
    assert_eq!(total, 214_748_364_800);
    assert_eq!(used, 10_737_418_240);
    assert_eq!(avail, 204_010_946_560);
    assert!((pct - 5.0).abs() < 0.01);
    assert_eq!(mnt, "/root/autodl-fs");

    let df_inodes = "\
Filesystem     Inodes  IUsed  IFree IUse% Mounted on
10.0.0.1:/fs   200000  45000 155000   23% /root/autodl-fs
";
    let inodes = parse_df_inodes_output(df_inodes).unwrap();
    assert_eq!(inodes.total, Some(200_000));
    assert_eq!(inodes.used, Some(45_000));
    assert_eq!(inodes.free, Some(155_000));
    assert_eq!(inodes.percent, Some(23.0));

    let df_inodes_dash = "\
Filesystem     Inodes  IUsed  IFree IUse% Mounted on
overlay             -      -      -     - /
";
    let inodes_dash = parse_df_inodes_output(df_inodes_dash).unwrap();
    assert_eq!(
        (
            inodes_dash.total,
            inodes_dash.used,
            inodes_dash.free,
            inodes_dash.percent
        ),
        (None, None, None, None)
    );

    let du_out = "1048576\t/root/autodl-fs/data\n4096\t/root/autodl-fs/notes.txt\n";
    assert_eq!(
        parse_du_output(du_out),
        vec![
            DuEntry {
                bytes: 1_048_576,
                path: "/root/autodl-fs/data".into()
            },
            DuEntry {
                bytes: 4096,
                path: "/root/autodl-fs/notes.txt".into()
            },
        ]
    );

    let cache_out = "d\t204800\t/root/autodl-fs/.Trash-0\nf\t1024\t/root/autodl-fs/.upload.tmp\n";
    assert_eq!(
        parse_cache_list_output(cache_out),
        vec![
            HiddenCacheEntry {
                is_dir: true,
                size_bytes: 204_800,
                path: "/root/autodl-fs/.Trash-0".into()
            },
            HiddenCacheEntry {
                is_dir: false,
                size_bytes: 1024,
                path: "/root/autodl-fs/.upload.tmp".into()
            },
        ]
    );
}

#[allow(deprecated)]
#[test]
fn test_fs_and_disk_help_output() {
    Command::cargo_bin("autodl")
        .unwrap()
        .args(["fs", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("clean-cache"))
        .stdout(predicate::str::contains("status"))
        .stdout(predicate::str::contains("du"));

    Command::cargo_bin("autodl")
        .unwrap()
        .args(["disk", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--du"))
        .stdout(predicate::str::contains("/root/autodl-fs"));
}
