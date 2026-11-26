use std::fs;
use std::io::Cursor;

use flate2::read::GzDecoder;
use tar::Archive;
use tempfile::TempDir;

use autodl_cli::api::types::ProInstanceSnapshot;
use autodl_cli::catalog::{parse_cuda_version, resolve_gpu_spec, resolve_image_uuid};
use autodl_cli::ssh::SshEndpoint;
use autodl_cli::ssh::transfer::{PackOptions, pack_directory_tar_gz};
use autodl_cli::workflow::{build_start_command, parse_download_spec};

#[test]
fn test_pack_directory_respects_gitignore_autodlignore_and_defaults() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    fs::write(root.join(".gitignore"), "*.log\nignored_dir/\n").unwrap();
    fs::write(root.join(".autodlignore"), "checkpoints/\n").unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/train.py"), "print('hello')\n").unwrap();
    fs::write(root.join("train.log"), "secret log").unwrap();
    fs::create_dir_all(root.join("ignored_dir")).unwrap();
    fs::write(root.join("ignored_dir/a.txt"), "skip").unwrap();
    fs::create_dir_all(root.join("checkpoints")).unwrap();
    fs::write(root.join("checkpoints/model.pt"), "weights").unwrap();
    fs::create_dir_all(root.join("__pycache__")).unwrap();
    fs::write(root.join("__pycache__/x.pyc"), "bytecode").unwrap();
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join(".git/HEAD"), "ref: refs/heads/main").unwrap();

    let (bytes, stats) = pack_directory_tar_gz(root, &PackOptions::default()).unwrap();
    assert!(stats.files >= 1);

    let mut archive = Archive::new(GzDecoder::new(Cursor::new(bytes)));
    let entries: Vec<String> = archive
        .entries()
        .unwrap()
        .map(|e| e.unwrap().path().unwrap().to_string_lossy().replace('\\', "/"))
        .collect();

    assert!(entries.iter().any(|p| p == "src/train.py"), "entries: {entries:?}");
    assert!(!entries.iter().any(|p| p.contains("train.log")), "entries: {entries:?}");
    assert!(!entries.iter().any(|p| p.contains("ignored_dir")), "entries: {entries:?}");
    assert!(!entries.iter().any(|p| p.contains("checkpoints")), "entries: {entries:?}");
    assert!(!entries.iter().any(|p| p.contains("__pycache__")), "entries: {entries:?}");
    assert!(!entries.iter().any(|p| p.contains(".git/")), "entries: {entries:?}");
}

#[test]
fn test_build_start_command_wrapping_and_auto_shutdown() {
    let dir = TempDir::new().unwrap();
    let py = dir.path().join("job.py");
    fs::write(&py, "print(42)\n").unwrap();

    let cmd = build_start_command(None, Some(&py), true, false)
        .unwrap()
        .unwrap();
    assert!(cmd.starts_with("sh -c "));
    assert!(cmd.contains("base64 -d > /tmp/autodl_start_script"));
    assert!(cmd.contains("python3 /tmp/autodl_start_script"));
    assert!(cmd.contains("/var/log/autodl-start.log"));
    assert!(cmd.contains("shutdown -h now || poweroff"));

    let raw = build_start_command(Some("sleep 1"), None, false, true)
        .unwrap()
        .unwrap();
    assert_eq!(raw, "sleep 1");
}

#[test]
fn test_ssh_endpoint_extraction_and_catalog_helpers() {
    let snap = ProInstanceSnapshot {
        proxy_host: String::new(),
        ssh_port: 0,
        ssh_command: "ssh -p 41234 root@region-9.autodl.com".into(),
        root_password: "pw!".into(),
        ..Default::default()
    };
    let ep = SshEndpoint::from_snapshot(&snap).unwrap();
    assert_eq!(ep.host, "region-9.autodl.com");
    assert_eq!(ep.port, 41234);
    assert_eq!(ep.user, "root");
    assert_eq!(ep.password, "pw!");

    assert_eq!(resolve_gpu_spec("4090"), ("v-48g".into(), true));
    assert_eq!(resolve_gpu_spec("H800"), ("h800".into(), true));
    assert_eq!(resolve_gpu_spec("custom-gpu"), ("custom-gpu".into(), false));

    let (img, cuda) = resolve_image_uuid("pytorch2.0");
    assert_eq!(img, "base-image-l2t43iu6uk");
    assert_eq!(cuda, Some(118));

    assert_eq!(parse_cuda_version("11.8").unwrap(), 118);
    assert_eq!(parse_cuda_version("12.1").unwrap(), 121);
    assert_eq!(parse_cuda_version("118").unwrap(), 118);

    let (r, l) = parse_download_spec("/root/autodl-tmp/out.pt:C:\\tmp\\out.pt").unwrap();
    assert_eq!(r, "/root/autodl-tmp/out.pt");
    assert_eq!(l.to_string_lossy(), "C:\\tmp\\out.pt");
}
