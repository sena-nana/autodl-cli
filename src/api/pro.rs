use std::time::{Duration, Instant};

use crate::error::AppError;

use super::AutoDlClient;
use super::types::{
    CreateProInstanceReq, InstanceUuidReq, PageReq, PagedList, PowerOnReq, ProInstanceSnapshot,
    ProInstanceSummary, ProPrivateImage, SaveProImageReq, SaveProImageResp,
};

impl AutoDlClient {
    /// `POST /api/v1/dev/instance/pro/create` -> returns newly created `instance_uuid` (`pro-...`).
    pub async fn create_pro_instance(
        &self,
        req: &CreateProInstanceReq,
    ) -> Result<String, AppError> {
        self.post_json("/api/v1/dev/instance/pro/create", req).await
    }

    async fn get_pro_instance_resource<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        instance_uuid: &str,
    ) -> Result<T, AppError> {
        let req = InstanceUuidReq { instance_uuid };
        match self.get_query(path, &req).await {
            Ok(val) => Ok(val),
            Err(AppError::Api { ref code, .. }) if code == "RequestParameterIsWrong" => {
                self.get_with_json_body(path, &req).await
            }
            Err(e) => Err(e),
        }
    }

    /// `GET /api/v1/dev/instance/pro/snapshot` (query param `instance_uuid`, with JSON body fallback).
    pub async fn get_pro_snapshot(
        &self,
        instance_uuid: &str,
    ) -> Result<ProInstanceSnapshot, AppError> {
        self.get_pro_instance_resource("/api/v1/dev/instance/pro/snapshot", instance_uuid)
            .await
    }

    /// `GET /api/v1/dev/instance/pro/status` (query param `instance_uuid`, with JSON body fallback).
    pub async fn get_pro_status(&self, instance_uuid: &str) -> Result<String, AppError> {
        self.get_pro_instance_resource("/api/v1/dev/instance/pro/status", instance_uuid)
            .await
    }

    /// `POST /api/v1/dev/instance/pro/list`
    pub async fn list_pro_instances(
        &self,
        page_index: u32,
        page_size: u32,
    ) -> Result<PagedList<ProInstanceSummary>, AppError> {
        self.post_json(
            "/api/v1/dev/instance/pro/list",
            &PageReq {
                page_index,
                page_size,
            },
        )
        .await
    }

    /// Fetch all pages of Pro instances.
    pub async fn list_all_pro_instances(&self) -> Result<Vec<ProInstanceSummary>, AppError> {
        let mut all = Vec::new();
        let mut page = 1u32;
        let page_size = 50u32;
        loop {
            let res = self.list_pro_instances(page, page_size).await?;
            all.extend(res.list);
            if page >= res.max_page || res.max_page == 0 {
                break;
            }
            page += 1;
            if page > 100 {
                break;
            }
        }
        Ok(all)
    }

    /// `POST /api/v1/dev/instance/pro/power_on` (`payload` is fixed to `"gpu"` per AutoDL docs).
    pub async fn power_on_pro(
        &self,
        instance_uuid: &str,
        start_command: Option<String>,
    ) -> Result<(), AppError> {
        let req = PowerOnReq {
            instance_uuid: instance_uuid.to_string(),
            payload: "gpu".to_string(),
            start_command,
        };
        self.post_json_unit("/api/v1/dev/instance/pro/power_on", &req)
            .await
    }

    /// `POST /api/v1/dev/instance/pro/power_off`
    pub async fn power_off_pro(&self, instance_uuid: &str) -> Result<(), AppError> {
        self.post_json_unit(
            "/api/v1/dev/instance/pro/power_off",
            &InstanceUuidReq { instance_uuid },
        )
        .await
    }

    /// `POST /api/v1/dev/instance/pro/release`
    /// Note: AutoDL requires the instance to be powered off first.
    pub async fn release_pro(&self, instance_uuid: &str) -> Result<(), AppError> {
        self.post_json_unit(
            "/api/v1/dev/instance/pro/release",
            &InstanceUuidReq { instance_uuid },
        )
        .await
    }

    /// Poll `get_pro_status` until the status matches any value in `targets` (or fails/times out).
    pub async fn wait_for_pro_status(
        &self,
        instance_uuid: &str,
        targets: &[&str],
        timeout: Duration,
        poll_interval: Duration,
    ) -> Result<String, AppError> {
        let deadline = Instant::now() + timeout;
        loop {
            let status = self.get_pro_status(instance_uuid).await?;
            let normalized = status.trim().to_ascii_lowercase();
            if targets
                .iter()
                .any(|t| normalized == t.to_ascii_lowercase())
            {
                return Ok(status);
            }
            if matches!(
                normalized.as_str(),
                "create_failed" | "failed" | "error" | "released"
            ) && !targets
                .iter()
                .any(|t| normalized == t.to_ascii_lowercase())
            {
                return Err(AppError::Other(format!(
                    "Instance '{instance_uuid}' entered terminal state '{status}'"
                )));
            }
            if Instant::now() >= deadline {
                return Err(AppError::Timeout(format!(
                    "Timed out after {}s waiting for instance '{instance_uuid}' to reach {:?} (last status: '{status}')",
                    timeout.as_secs(),
                    targets
                )));
            }
            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Safely power off (if needed) and release an AutoDL Pro instance.
    pub async fn stop_and_release_pro(
        &self,
        instance_uuid: &str,
        timeout: Duration,
    ) -> Result<(), AppError> {
        let status = self.get_pro_status(instance_uuid).await?;
        let lower = status.trim().to_ascii_lowercase();
        if lower == "released" {
            return Ok(());
        }
        if lower != "shutdown" && lower != "stopped" {
            if lower != "shutting_down" && lower != "stopping" {
                self.power_off_pro(instance_uuid).await?;
            }
            self.wait_for_pro_status(
                instance_uuid,
                &["shutdown", "stopped"],
                timeout,
                Duration::from_secs(2),
            )
            .await?;
        }
        self.release_pro(instance_uuid).await
    }

    /// `POST /api/v1/dev/instance/pro/image/save`
    pub async fn save_pro_image(
        &self,
        instance_uuid: &str,
        image_name: &str,
    ) -> Result<SaveProImageResp, AppError> {
        self.post_json(
            "/api/v1/dev/instance/pro/image/save",
            &SaveProImageReq {
                instance_uuid: instance_uuid.to_string(),
                image_name: image_name.to_string(),
            },
        )
        .await
    }

    /// `POST /api/v1/dev/instance/pro/image/private/list`
    pub async fn list_pro_private_images(
        &self,
        page_index: u32,
        page_size: u32,
    ) -> Result<PagedList<ProPrivateImage>, AppError> {
        self.post_json(
            "/api/v1/dev/instance/pro/image/private/list",
            &PageReq {
                page_index,
                page_size,
            },
        )
        .await
    }
}
