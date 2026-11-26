use std::fs;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[allow(deprecated)]
fn autodl_cmd(cfg: &TempDir, cred_file: &std::path::Path, api_base: &str) -> Command {
    let mut cmd = Command::cargo_bin("autodl").unwrap();
    cmd.env("AUTODL_CONFIG_DIR", cfg.path())
        .env("AUTODL_CREDENTIAL_FILE", cred_file)
        .env("AUTODL_API_BASE", api_base)
        .env_remove("AUTODL_TOKEN")
        .env_remove("AUTODL_PROFILE");
    cmd
}

#[tokio::test]
async fn test_auth_login_switch_status_and_full_logout() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/dev/wallet/balance"))
        .and(header("Authorization", "secret-token-alpha-1234"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": "Success",
            "data": {"assets": 50000, "accumulate": 1000, "voucher_balance": 0},
            "msg": ""
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/dev/wallet/balance"))
        .and(header("Authorization", "secret-token-beta-5678"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": "Success",
            "data": {"assets": 80000, "accumulate": 2000, "voucher_balance": 500},
            "msg": ""
        })))
        .mount(&server)
        .await;

    let cfg = TempDir::new().unwrap();
    let cred_file = cfg.path().join("creds.json");

    // 1. Login default profile
    let out = autodl_cmd(&cfg, &cred_file, &server.uri())
        .args(["auth", "login", "--token", "secret-token-alpha-1234", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["profile"], "default");
    assert_eq!(v["verified"], true);

    // 2. Login work profile
    let out = autodl_cmd(&cfg, &cred_file, &server.uri())
        .args([
            "--profile",
            "work",
            "auth",
            "login",
            "--token",
            "secret-token-beta-5678",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());

    // 3. List profiles
    let out = autodl_cmd(&cfg, &cred_file, &server.uri())
        .args(["auth", "list", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["active_profile"], "work");
    assert_eq!(v["profiles"].as_array().unwrap().len(), 2);

    // 4. Switch back to default and check usage
    let out = autodl_cmd(&cfg, &cred_file, &server.uri())
        .args(["auth", "switch", "default", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());

    let out = autodl_cmd(&cfg, &cred_file, &server.uri())
        .args(["usage", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["assets_milli"], 50000);

    // 5. Seed a local session in sessions.json to verify logout clears it cleanly
    let sessions_path = cfg.path().join("sessions.json");
    fs::write(
        &sessions_path,
        serde_json::json!({
            "sessions": {
                "dev-1": {
                    "name": "dev-1",
                    "instance_uuid": "pro-deadbeef",
                    "profile": "default",
                    "gpu_spec": "v-48g",
                    "gpu_count": 1,
                    "image_uuid": "base-image-l2t43iu6uk",
                    "created_at": "2026-01-01T00:00:00Z",
                    "default_remote_dir": "/root/autodl-tmp"
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    // 6. Logout default profile (top-level `autodl logout`)
    let out = autodl_cmd(&cfg, &cred_file, &server.uri())
        .args(["logout", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["credentials_deleted"], 1);
    assert_eq!(v["sessions_cleared"], 1);
    assert_eq!(v["next_active_profile"], "work");

    // 7. Logout remaining profiles with --all and verify status reports logged_in=false
    let out = autodl_cmd(&cfg, &cred_file, &server.uri())
        .args(["auth", "logout", "--all", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());

    let out = autodl_cmd(&cfg, &cred_file, &server.uri())
        .args(["auth", "status", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["logged_in"], false);

    // Unauthenticated usage call must exit with code 3
    let out = autodl_cmd(&cfg, &cred_file, &server.uri())
        .args(["usage", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
}

#[tokio::test]
async fn test_session_new_boot_script_and_auto_resolve_stop() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/dev/wallet/balance"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": "Success",
            "data": {"assets": 10000, "accumulate": 0, "voucher_balance": 0},
            "msg": ""
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/dev/instance/pro/create"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": "Success",
            "data": "pro-sess001",
            "msg": ""
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/dev/instance/pro/list"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": "Success",
            "data": {
                "list": [{
                    "uuid": "pro-sess001",
                    "name": "trainer",
                    "status": "running",
                    "region_sign": "westDC2",
                    "req_gpu_amount": 1,
                    "gpu_spec_uuid": "v-48g"
                }],
                "page_index": 1,
                "page_size": 50,
                "max_page": 1,
                "result_total": 1
            },
            "msg": ""
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v1/dev/instance/pro/status"))
        .and(body_json(serde_json::json!({"instance_uuid": "pro-sess001"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": "Success",
            "data": "running",
            "msg": ""
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v1/dev/instance/pro/snapshot"))
        .and(body_json(serde_json::json!({"instance_uuid": "pro-sess001"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": "Success",
            "data": {
                "region_sign": "westDC2",
                "payg_price": 1970,
                "origin_pay_price": 3030,
                "snapshot_gpu_alias_name": "4090-48G",
                "ssh_command": "ssh -p 22222 root@connect.west.autodl.com",
                "proxy_host": "connect.west.autodl.com",
                "root_password": "secret",
                "ssh_port": 22222,
                "jupyter_token": "jt",
                "jupyter_domain": "j.autodl.com:8443"
            },
            "msg": ""
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/dev/instance/pro/power_off"))
        .and(body_json(serde_json::json!({"instance_uuid": "pro-sess001"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": "Success",
            "data": null,
            "msg": ""
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v1/dev/instance/pro/status"))
        .and(body_json(serde_json::json!({"instance_uuid": "pro-sess001"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": "Success",
            "data": "shutdown",
            "msg": ""
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/dev/instance/pro/release"))
        .and(body_json(serde_json::json!({"instance_uuid": "pro-sess001"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": "Success",
            "data": null,
            "msg": ""
        })))
        .expect(1)
        .mount(&server)
        .await;

    let cfg = TempDir::new().unwrap();
    let cred_file = cfg.path().join("creds.json");
    let boot_script = cfg.path().join("boot.py");
    fs::write(&boot_script, "print('boot ok')\n").unwrap();

    // Login
    let out = autodl_cmd(&cfg, &cred_file, &server.uri())
        .args(["auth", "login", "--token", "tok-session-1234", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());

    // Create instance with --start-script and --auto-shutdown-on-exit
    let out = autodl_cmd(&cfg, &cred_file, &server.uri())
        .args([
            "new",
            "-s",
            "trainer",
            "--gpu",
            "4090",
            "--start-script",
            boot_script.to_str().unwrap(),
            "--auto-shutdown-on-exit",
            "--no-wait",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["instance_uuid"], "pro-sess001");
    assert_eq!(v["gpu_spec"], "v-48g");
    assert_eq!(v["has_start_command"], true);

    // Single-session auto-resolution for `url --json` without `-s`
    let out = autodl_cmd(&cfg, &cred_file, &server.uri())
        .args(["url", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["jupyter_url"], "https://j.autodl.com:8443/?token=jt");

    // Single-session auto-resolution for `stop --yes --json` without `-s`
    let out = autodl_cmd(&cfg, &cred_file, &server.uri())
        .args(["stop", "--yes", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["released"], true);
}

#[test]
fn test_catalog_commands_json() {
    let cfg = TempDir::new().unwrap();
    let cred_file = cfg.path().join("creds.json");

    for sub in ["gpus", "images", "regions"] {
        let out = autodl_cmd(&cfg, &cred_file, "http://127.0.0.1:1")
            .args(["catalog", sub, "--json"])
            .output()
            .unwrap();
        assert!(out.status.success());
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["ok"], true);
    }
}
