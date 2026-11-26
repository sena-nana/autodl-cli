use crate::error::AppError;

use super::AutoDlClient;
use super::types::{ExclusiveNfsMountReq, WalletBalance};

impl AutoDlClient {
    /// `POST /api/v1/dev/wallet/balance`
    pub async fn wallet_balance(&self) -> Result<WalletBalance, AppError> {
        self.post_empty("/api/v1/dev/wallet/balance").await
    }

    /// `POST /api/v1/dev/exclusive_nfs/mount`
    /// `mountable`: `1` to mount exclusive NFS, `-1` to switch back to standard file storage.
    pub async fn exclusive_nfs_mount(&self, data_center: &str, mount: bool) -> Result<(), AppError> {
        let req = ExclusiveNfsMountReq {
            data_center: data_center.to_string(),
            mountable: if mount { 1 } else { -1 },
        };
        self.post_json_unit("/api/v1/dev/exclusive_nfs/mount", &req)
            .await
    }
}
