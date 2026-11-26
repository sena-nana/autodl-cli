use std::time::Duration;

use serde_json::json;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use autodl_cli::api::AutoDlClient;
use autodl_cli::api::types::{
    AddBlacklistReq, ContainerTemplate, CreateDeploymentReq, CreateProInstanceReq, GpuStockReq,
};
use autodl_cli::error::AppError;

#[tokio::test]
async fn test_wallet_balance_and_nfs_mount() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/dev/wallet/balance"))
        .and(header("Authorization", "tok-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "Success",
            "data": {
                "assets": 12500,
                "accumulate": 45000,
                "voucher_balance": 3000
            },
            "msg": ""
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/dev/exclusive_nfs/mount"))
        .and(header("Authorization", "tok-123"))
        .and(body_json(json!({
            "data_center": "westDC2",
            "mountable": 1
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "Success",
            "msg": ""
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = AutoDlClient::new("tok-123".into(), Some(&server.uri())).unwrap();
    let bal = client.wallet_balance().await.unwrap();
    assert_eq!(bal.assets, 12500);
    assert_eq!(bal.accumulate, 45000);
    assert_eq!(bal.voucher_balance, 3000);

    let reqs = server.received_requests().await.unwrap();
    let balance_req = reqs
        .iter()
        .find(|r| r.url.path() == "/api/v1/dev/wallet/balance")
        .unwrap();
    assert!(
        !balance_req.headers.contains_key("content-type"),
        "bodyless wallet/balance POST should not send Content-Type header"
    );

    client.exclusive_nfs_mount("westDC2", true).await.unwrap();
}

#[tokio::test]
async fn test_pro_instance_lifecycle_and_get_with_json_body() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/dev/instance/pro/create"))
        .and(header("Authorization", "tok-pro"))
        .and(body_json(json!({
            "data_center_list": ["westDC3"],
            "req_gpu_amount": 1,
            "expand_system_disk_by_gb": 10,
            "gpu_spec_uuid": "v-48g",
            "image_uuid": "base-image-l2t43iu6uk",
            "cuda_v_from": 118,
            "instance_name": "train-job",
            "start_command": "python3 train.py"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "Success",
            "data": "pro-abc123",
            "msg": "",
            "request_id": "req-1"
        })))
        .expect(1)
        .mount(&server)
        .await;

    // AutoDL snapshot & status use GET with JSON body
    Mock::given(method("GET"))
        .and(path("/api/v1/dev/instance/pro/snapshot"))
        .and(header("Authorization", "tok-pro"))
        .and(body_json(json!({"instance_uuid": "pro-abc123"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "Success",
            "data": {
                "region_sign": "westDC3",
                "payg_price": 1970,
                "origin_pay_price": 3030,
                "snapshot_gpu_alias_name": "RTX 4090",
                "ssh_command": "ssh -p 34222 root@connect.west.autodl.com",
                "proxy_host": "connect.west.autodl.com",
                "root_password": "pw+",
                "ssh_port": 34222,
                "jupyter_token": "jtok",
                "jupyter_domain": "a1.autodl.com:8443"
            },
            "msg": "",
            "request_id": "req-2"
        })))
        .expect(1)
        .mount(&server)
        .await;

    // Multi-page list
    Mock::given(method("POST"))
        .and(path("/api/v1/dev/instance/pro/list"))
        .and(body_json(json!({"page_index": 1, "page_size": 50})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "Success",
            "data": {
                "list": [{
                    "uuid": "pro-abc123",
                    "name": "train-job",
                    "status": "running",
                    "region_sign": "westDC3",
                    "req_gpu_amount": 1,
                    "gpu_spec_uuid": "v-48g",
                    "created_at": "2026-01-01T00:00:00+08:00",
                    "started_at": {"Time": "2026-01-01T00:01:00+08:00", "Valid": true},
                    "stopped_at": {"Time": "0001-01-01T00:00:00Z", "Valid": false}
                }],
                "page_index": 1,
                "page_size": 50,
                "max_page": 2,
                "result_total": 2
            },
            "msg": ""
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/dev/instance/pro/list"))
        .and(body_json(json!({"page_index": 2, "page_size": 50})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "Success",
            "data": {
                "list": [{
                    "uuid": "pro-def456",
                    "name": "eval-job",
                    "status": "shutdown",
                    "region_sign": "beijingDC2",
                    "req_gpu_amount": 2,
                    "gpu_spec_uuid": "h800"
                }],
                "page_index": 2,
                "page_size": 50,
                "max_page": 2,
                "result_total": 2
            },
            "msg": ""
        })))
        .expect(1)
        .mount(&server)
        .await;

    // stop_and_release_pro sequence: status("running") -> power_off -> status("shutdown") -> release
    Mock::given(method("GET"))
        .and(path("/api/v1/dev/instance/pro/status"))
        .and(body_json(json!({"instance_uuid": "pro-abc123"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "Success",
            "data": "running",
            "msg": ""
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/dev/instance/pro/power_off"))
        .and(body_json(json!({"instance_uuid": "pro-abc123"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "Success",
            "data": null,
            "msg": ""
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v1/dev/instance/pro/status"))
        .and(body_json(json!({"instance_uuid": "pro-abc123"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "Success",
            "data": "shutdown",
            "msg": ""
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/dev/instance/pro/release"))
        .and(body_json(json!({"instance_uuid": "pro-abc123"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "Success",
            "data": null,
            "msg": ""
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = AutoDlClient::new("tok-pro".into(), Some(&server.uri())).unwrap();
    let uuid = client
        .create_pro_instance(&CreateProInstanceReq {
            data_center_list: Some(vec!["westDC3".into()]),
            req_gpu_amount: 1,
            expand_system_disk_by_gb: 10,
            gpu_spec_uuid: "v-48g".into(),
            image_uuid: "base-image-l2t43iu6uk".into(),
            cuda_v_from: 118,
            instance_name: Some("train-job".into()),
            start_command: Some("python3 train.py".into()),
        })
        .await
        .unwrap();
    assert_eq!(uuid, "pro-abc123");

    let snap = client.get_pro_snapshot(&uuid).await.unwrap();
    assert_eq!(snap.ssh_port, 34222);
    assert_eq!(snap.proxy_host, "connect.west.autodl.com");

    let all = client.list_all_pro_instances().await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].uuid, "pro-abc123");
    assert!(all[0].started_at.valid);
    assert!(!all[0].stopped_at.valid);
    assert_eq!(all[1].uuid, "pro-def456");

    client
        .stop_and_release_pro(&uuid, Duration::from_secs(10))
        .await
        .unwrap();
}

#[tokio::test]
async fn test_power_on_images_and_error_mapping() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/dev/instance/pro/power_on"))
        .and(body_json(json!({
            "instance_uuid": "pro-111",
            "payload": "gpu",
            "start_command": "echo boot"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "Success",
            "data": null,
            "msg": ""
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/dev/instance/pro/image/save"))
        .and(body_json(json!({
            "instance_uuid": "pro-111",
            "image_name": "ckpt-v1"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "Success",
            "data": {"image_uuid": "image-999"},
            "msg": ""
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/dev/wallet/balance"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "code": "AuthorizeFailed",
            "msg": "invalid developer token",
            "request_id": "req-auth-err"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = AutoDlClient::new("bad-tok".into(), Some(&server.uri())).unwrap();
    client
        .power_on_pro("pro-111", Some("echo boot".into()))
        .await
        .unwrap();
    let saved = client.save_pro_image("pro-111", "ckpt-v1").await.unwrap();
    assert_eq!(saved.image_uuid, "image-999");

    let err = client.wallet_balance().await.unwrap_err();
    assert_eq!(err.exit_code(), 3);
    match err {
        AppError::Api {
            code,
            request_id,
            http_status,
            ..
        } => {
            assert_eq!(code, "AuthorizeFailed");
            assert_eq!(request_id.as_deref(), Some("req-auth-err"));
            assert_eq!(http_status, Some(401));
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[tokio::test]
async fn test_elastic_deployment_and_gpu_stock() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/dev/machine/region/gpu_stock"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "Success",
            "data": [
                {"RTX 4090": {"idle_gpu_num": 120, "total_gpu_num": 500}}
            ],
            "msg": ""
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/dev/deployment"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "Success",
            "data": {"deployment_uuid": "dep-xyz"},
            "msg": ""
        })))
        .mount(&server)
        .await;

    Mock::given(method("PUT"))
        .and(path("/api/v1/dev/deployment/operate"))
        .and(body_json(json!({"deployment_uuid": "dep-xyz", "operate": "stop"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "Success",
            "data": null,
            "msg": ""
        })))
        .mount(&server)
        .await;

    Mock::given(method("DELETE"))
        .and(path("/api/v1/dev/deployment"))
        .and(body_json(json!({"deployment_uuid": "dep-xyz"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "Success",
            "data": null,
            "msg": ""
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/dev/deployment/blacklist"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "Success",
            "data": null,
            "msg": ""
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v1/dev/deployment/ddp/overview"))
        .and(query_param("deployment_uuid", "dep-xyz"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "Success",
            "data": [{
                "gpu_type": "RTX 4090",
                "total": 86400,
                "balance": 80000,
                "dc_list": "westDC2"
            }],
            "msg": ""
        })))
        .mount(&server)
        .await;

    let client = AutoDlClient::new("tok-esd".into(), Some(&server.uri())).unwrap();

    let stock = client
        .gpu_stock(&GpuStockReq {
            region_sign: "westDC2".into(),
            cuda_v_from: Some(118),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(stock[0]["RTX 4090"].idle_gpu_num, 120);

    let dep = client
        .create_deployment(&CreateDeploymentReq {
            name: "test-dep".into(),
            deployment_type: "Container".into(),
            replica_num: None,
            parallelism_num: None,
            reuse_container: Some(true),
            reuse_container_scope: None,
            container_template: ContainerTemplate {
                dc_list: vec!["westDC2".into()],
                gpu_name_set: vec!["RTX 4090".into()],
                gpu_num: 1,
                cuda_v_from: 118,
                cuda_v_to: 128,
                cpu_num_from: 1,
                cpu_num_to: 64,
                memory_size_from: 1,
                memory_size_to: 256,
                price_from: 100,
                price_to: 9000,
                image_uuid: "base-image-l2t43iu6uk".into(),
                cmd: "sleep 100".into(),
                cmd_before_shutdown: None,
                service_6006_port_protocol: None,
                service_6008_port_protocol: None,
            },
        })
        .await
        .unwrap();
    assert_eq!(dep.deployment_uuid, "dep-xyz");

    client
        .add_deployment_blacklist(&AddBlacklistReq {
            deployment_container_uuid: "c-1".into(),
            expire_in_minutes: Some(60),
            comment: Some("slow".into()),
        })
        .await
        .unwrap();

    let ddp = client.deployment_ddp_overview("dep-xyz").await.unwrap();
    assert_eq!(ddp[0].balance, 80000);

    client.stop_deployment("dep-xyz").await.unwrap();
    client.delete_deployment("dep-xyz").await.unwrap();
}
