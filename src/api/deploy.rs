use serde::Serialize;

use crate::error::AppError;

use super::AutoDlClient;
use super::types::{
    AddBlacklistReq, BlacklistEntry, ContainerEvent, ContainerEventListReq, CreateDeploymentReq,
    CreateDeploymentResp, DdpOverviewItem, DeleteDeploymentReq, DeploymentContainer,
    DeploymentContainerListReq, DeploymentListReq, DeploymentSummary, ElasticPrivateImage,
    GpuStockItem, GpuStockReq, OperateDeploymentReq, PageReq, PagedList, SetReplicaNumReq,
    StopContainerReq,
};

#[derive(Serialize)]
struct DdpQuery<'a> {
    deployment_uuid: &'a str,
}

impl AutoDlClient {
    /// `POST /api/v1/dev/machine/region/gpu_stock`
    pub async fn gpu_stock(&self, req: &GpuStockReq) -> Result<Vec<GpuStockItem>, AppError> {
        self.post_json_or_default("/api/v1/dev/machine/region/gpu_stock", req)
            .await
    }

    /// `POST /api/v1/dev/image/private/list` (Elastic deployment private images)
    pub async fn list_elastic_images(
        &self,
        page_index: u32,
        page_size: u32,
    ) -> Result<PagedList<ElasticPrivateImage>, AppError> {
        self.post_json(
            "/api/v1/dev/image/private/list",
            &PageReq {
                page_index,
                page_size,
            },
        )
        .await
    }

    /// `POST /api/v1/dev/deployment`
    pub async fn create_deployment(
        &self,
        req: &CreateDeploymentReq,
    ) -> Result<CreateDeploymentResp, AppError> {
        self.post_json("/api/v1/dev/deployment", req).await
    }

    /// `POST /api/v1/dev/deployment/list`
    pub async fn list_deployments(
        &self,
        req: &DeploymentListReq,
    ) -> Result<PagedList<DeploymentSummary>, AppError> {
        self.post_json("/api/v1/dev/deployment/list", req).await
    }

    /// `POST /api/v1/dev/deployment/container/list`
    pub async fn list_deployment_containers(
        &self,
        req: &DeploymentContainerListReq,
    ) -> Result<PagedList<DeploymentContainer>, AppError> {
        self.post_json("/api/v1/dev/deployment/container/list", req)
            .await
    }

    /// `POST /api/v1/dev/deployment/container/event/list`
    pub async fn list_deployment_container_events(
        &self,
        req: &ContainerEventListReq,
    ) -> Result<PagedList<ContainerEvent>, AppError> {
        self.post_json("/api/v1/dev/deployment/container/event/list", req)
            .await
    }

    /// `PUT /api/v1/dev/deployment/container/stop`
    pub async fn stop_deployment_container(&self, req: &StopContainerReq) -> Result<(), AppError> {
        self.put_json_unit("/api/v1/dev/deployment/container/stop", req)
            .await
    }

    /// `PUT /api/v1/dev/deployment/replica_num`
    pub async fn set_deployment_replica_num(
        &self,
        deployment_uuid: &str,
        replica_num: u32,
    ) -> Result<(), AppError> {
        self.put_json_unit(
            "/api/v1/dev/deployment/replica_num",
            &SetReplicaNumReq {
                deployment_uuid: deployment_uuid.to_string(),
                replica_num,
            },
        )
        .await
    }

    /// `PUT /api/v1/dev/deployment/operate`
    pub async fn stop_deployment(&self, deployment_uuid: &str) -> Result<(), AppError> {
        self.put_json_unit(
            "/api/v1/dev/deployment/operate",
            &OperateDeploymentReq {
                deployment_uuid: deployment_uuid.to_string(),
                operate: "stop".to_string(),
            },
        )
        .await
    }

    /// `DELETE /api/v1/dev/deployment`
    pub async fn delete_deployment(&self, deployment_uuid: &str) -> Result<(), AppError> {
        self.delete_json_unit(
            "/api/v1/dev/deployment",
            &DeleteDeploymentReq {
                deployment_uuid: deployment_uuid.to_string(),
            },
        )
        .await
    }

    /// `POST /api/v1/dev/deployment/blacklist`
    pub async fn add_deployment_blacklist(&self, req: &AddBlacklistReq) -> Result<(), AppError> {
        self.post_json_unit("/api/v1/dev/deployment/blacklist", req)
            .await
    }

    /// `GET /api/v1/dev/deployment/blacklist`
    pub async fn list_deployment_blacklist(&self) -> Result<Vec<BlacklistEntry>, AppError> {
        self.get_empty_or_default("/api/v1/dev/deployment/blacklist")
            .await
    }

    /// `GET /api/v1/dev/deployment/ddp/overview?deployment_uuid=...`
    pub async fn deployment_ddp_overview(
        &self,
        deployment_uuid: &str,
    ) -> Result<Vec<DdpOverviewItem>, AppError> {
        self.get_query_or_default(
            "/api/v1/dev/deployment/ddp/overview",
            &DdpQuery { deployment_uuid },
        )
        .await
    }
}
