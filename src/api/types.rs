use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiEnvelope<T> {
    pub code: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub msg: String,
    #[serde(default)]
    pub request_id: Option<String>,
    pub data: Option<T>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct PagedList<T> {
    #[serde(
        default,
        deserialize_with = "null_as_default",
        bound(deserialize = "T: Deserialize<'de>")
    )]
    pub list: Vec<T>,
    #[serde(default)]
    pub page_index: u32,
    #[serde(default)]
    pub page_size: u32,
    #[serde(default)]
    pub offset: u32,
    #[serde(default)]
    pub max_page: u32,
    #[serde(default)]
    pub result_total: u64,
    #[serde(default)]
    pub page: Option<u32>,
}

// ============================================================================
// Common API (/api/v1/dev/wallet/balance, /api/v1/dev/exclusive_nfs/mount)
// ============================================================================

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct WalletBalance {
    #[serde(default)]
    pub assets: i64,
    #[serde(default)]
    pub accumulate: i64,
    #[serde(default)]
    pub voucher_balance: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExclusiveNfsMountReq {
    pub data_center: String,
    pub mountable: i32,
}

// ============================================================================
// Container Instance Pro API (/api/v1/dev/instance/pro/*)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateProInstanceReq {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_center_list: Option<Vec<String>>,
    pub req_gpu_amount: u32,
    pub expand_system_disk_by_gb: u32,
    pub gpu_spec_uuid: String,
    pub image_uuid: String,
    pub cuda_v_from: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_command: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstanceUuidReq<'a> {
    pub instance_uuid: &'a str,
}

#[derive(Debug, Clone, Serialize)]
pub struct PowerOnReq {
    pub instance_uuid: String,
    pub payload: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_command: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PageReq {
    pub page_index: u32,
    pub page_size: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct GoNullTime {
    #[serde(rename = "Time", default, deserialize_with = "null_as_default")]
    pub time: String,
    #[serde(rename = "Valid", default)]
    pub valid: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ProInstanceSummary {
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub uuid: String,
    #[serde(default)]
    pub machine_id: String,
    #[serde(default)]
    pub machine_alias: String,
    #[serde(default)]
    pub region_sign: String,
    #[serde(default)]
    pub region_name: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub sub_status: String,
    #[serde(default)]
    pub status_at: String,
    #[serde(default)]
    pub start_mode: String,
    #[serde(default)]
    pub charge_type: String,
    #[serde(default)]
    pub req_gpu_amount: u32,
    #[serde(default)]
    pub expired_at: GoNullTime,
    #[serde(default)]
    pub started_at: GoNullTime,
    #[serde(default)]
    pub stopped_at: GoNullTime,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub timed_shutdown_at: GoNullTime,
    #[serde(default)]
    pub gpu_spec_uuid: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct UsageInfo {
    #[serde(default)]
    pub container_id: String,
    #[serde(default)]
    pub valid_at: String,
    #[serde(default)]
    pub cpu_usage_percent: f64,
    #[serde(default)]
    pub mem_usage_percent: f64,
    #[serde(default)]
    pub mem_usage: u64,
    #[serde(default)]
    pub mem_limit: u64,
    #[serde(default)]
    pub root_fs_used_size: u64,
    #[serde(default)]
    pub root_fs_total_size: u64,
    #[serde(default)]
    pub data_disk_total_size: u64,
    #[serde(default)]
    pub data_disk_used_size: u64,
    #[serde(default)]
    pub pull_image_progress: f64,
    #[serde(default)]
    pub download_image_progress: f64,
    #[serde(default)]
    pub download_oss_file_progress: f64,
    #[serde(default)]
    pub is_new: bool,
    #[serde(default)]
    pub valid: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ProInstanceSnapshot {
    #[serde(default)]
    pub region_sign: String,
    #[serde(default)]
    pub payg_price: i64,
    #[serde(default)]
    pub origin_pay_price: i64,
    #[serde(default)]
    pub snapshot_gpu_alias_name: String,
    #[serde(default)]
    pub chip_corp: String,
    #[serde(default)]
    pub cpu_arch: String,
    #[serde(default)]
    pub usage_info: Option<UsageInfo>,
    #[serde(default)]
    pub expand_system_disk_size: u64,
    #[serde(default)]
    pub system_init_disk_size: u64,
    #[serde(default)]
    pub ssh_command: String,
    #[serde(default)]
    pub proxy_host: String,
    #[serde(default)]
    pub root_password: String,
    #[serde(default)]
    pub ssh_port: u16,
    #[serde(default)]
    pub jupyter_token: String,
    #[serde(default)]
    pub jupyter_domain: String,
    #[serde(default)]
    pub service_6006_domain: String,
    #[serde(default)]
    pub service_6006_port_protocol: String,
    #[serde(default)]
    pub service_6008_domain: String,
    #[serde(default)]
    pub service_6008_port_protocol: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SaveProImageReq {
    pub instance_uuid: String,
    pub image_name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SaveProImageResp {
    #[serde(default)]
    pub image_uuid: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ProPrivateImage {
    #[serde(default)]
    pub image_uuid: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub image_size: u64,
    #[serde(default)]
    pub create_at: String,
}

// ============================================================================
// Elastic Deployment & GPU Stock API
// ============================================================================

#[derive(Debug, Clone, Serialize, Default)]
pub struct GpuStockReq {
    pub region_sign: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cuda_v_from: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cuda_v_to: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_name_set: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_size_from: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_size_to: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_num_from: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_num_to: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_from: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_to: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct GpuStockCounts {
    #[serde(default)]
    pub idle_gpu_num: u64,
    #[serde(default)]
    pub total_gpu_num: u64,
}

pub type GpuStockItem = BTreeMap<String, GpuStockCounts>;

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ElasticPrivateImage {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub image_uuid: String,
    #[serde(default)]
    pub image_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerTemplate {
    pub dc_list: Vec<String>,
    pub gpu_name_set: Vec<String>,
    pub gpu_num: u32,
    pub cuda_v_from: i32,
    pub cuda_v_to: i32,
    pub cpu_num_from: u32,
    pub cpu_num_to: u32,
    pub memory_size_from: u32,
    pub memory_size_to: u32,
    pub price_from: u32,
    pub price_to: u32,
    pub image_uuid: String,
    pub cmd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cmd_before_shutdown: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_6006_port_protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_6008_port_protocol: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateDeploymentReq {
    pub name: String,
    pub deployment_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replica_num: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallelism_num: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reuse_container: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reuse_container_scope: Option<String>,
    pub container_template: ContainerTemplate,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CreateDeploymentResp {
    #[serde(default)]
    pub deployment_uuid: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct DeploymentListReq {
    pub page_index: u32,
    pub page_size: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deployment_uuid: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct DeploymentSummary {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub uuid: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub deployment_type: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub replica_num: u32,
    #[serde(default)]
    pub parallelism_num: u32,
    #[serde(default)]
    pub starting_num: u32,
    #[serde(default)]
    pub running_num: u32,
    #[serde(default)]
    pub finished_num: u32,
    #[serde(default)]
    pub image_uuid: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct DeploymentContainerListReq {
    pub deployment_uuid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_uuid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub released: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<Vec<String>>,
    pub page_index: u32,
    pub page_size: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ContainerInfo {
    #[serde(default)]
    pub ssh_command: String,
    #[serde(default)]
    pub root_password: String,
    #[serde(default)]
    pub service_6006_port_url: String,
    #[serde(default)]
    pub service_6008_port_url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct DeploymentContainer {
    #[serde(default)]
    pub uuid: String,
    #[serde(default)]
    pub data_center: String,
    #[serde(default)]
    pub machine_id: String,
    #[serde(default)]
    pub deployment_uuid: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub gpu_name: String,
    #[serde(default)]
    pub gpu_num: u32,
    #[serde(default)]
    pub cpu_num: u32,
    #[serde(default)]
    pub memory_size: u64,
    #[serde(default)]
    pub image_uuid: String,
    #[serde(default)]
    pub price: f64,
    #[serde(default)]
    pub info: Option<ContainerInfo>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub stopped_at: Option<String>,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContainerEventListReq {
    pub deployment_uuid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deployment_container_uuid: Option<String>,
    pub page_index: u32,
    pub page_size: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ContainerEvent {
    #[serde(default)]
    pub deployment_container_uuid: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StopContainerReq {
    pub deployment_container_uuid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decrease_one_replica_num: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_cache: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cmd_before_shutdown: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SetReplicaNumReq {
    pub deployment_uuid: String,
    pub replica_num: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct OperateDeploymentReq {
    pub deployment_uuid: String,
    pub operate: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeleteDeploymentReq {
    pub deployment_uuid: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddBlacklistReq {
    pub deployment_container_uuid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expire_in_minutes: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct BlacklistEntry {
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub data_center: String,
    #[serde(default)]
    pub expired_time: String,
    #[serde(default)]
    pub machine_id: String,
    #[serde(default)]
    pub msg: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct DdpOverviewItem {
    #[serde(default)]
    pub gpu_type: String,
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub balance: u64,
    #[serde(default)]
    pub dc_list: String,
}
