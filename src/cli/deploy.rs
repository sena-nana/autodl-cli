use std::collections::BTreeMap;

use clap::{Args, Subcommand};

use crate::api::types::{
    AddBlacklistReq, ContainerEventListReq, ContainerTemplate, CreateDeploymentReq,
    DeploymentContainerListReq, DeploymentListReq, GpuStockReq, StopContainerReq,
};
use crate::catalog::{BASE_IMAGES, GPU_SPECS, REGIONS, parse_cuda_version};
use crate::error::AppError;
use crate::output::{format_bytes, make_table};
use crate::state::resolve_session;

use super::CliContext;

// ============================================================================
// Images
// ============================================================================

#[derive(Debug, Args)]
pub struct ImagesArgs {
    #[command(subcommand)]
    pub command: ImagesCommand,
}

#[derive(Debug, Subcommand)]
pub enum ImagesCommand {
    /// List private images (Pro API by default, or --elastic for Elastic Deployment API)
    List {
        #[arg(long)]
        elastic: bool,
        #[arg(long, default_value_t = 1)]
        page: u32,
        #[arg(long = "page-size", default_value_t = 20)]
        page_size: u32,
    },
    /// Save an existing Pro instance as a private image
    Save {
        #[arg(short = 's', long = "session")]
        session: Option<String>,
        #[arg(long)]
        name: String,
    },
}

pub async fn handle_images(ctx: &CliContext, args: ImagesArgs) -> Result<(), AppError> {
    let (client, profile) = ctx.api_client()?;
    let store = ctx.session_store();

    match args.command {
        ImagesCommand::List {
            elastic,
            page,
            page_size,
        } => {
            if elastic {
                let res = client.list_elastic_images(page, page_size).await?;
                if ctx.out.json {
                    ctx.out.emit_json(&serde_json::json!({
                        "ok": true,
                        "kind": "elastic",
                        "data": res,
                    }))?;
                    return Ok(());
                }
                if res.list.is_empty() {
                    ctx.out.println("No private images found.");
                    return Ok(());
                }
                let mut table = make_table(&["ID", "UUID", "Name"]);
                for img in res.list {
                    table.add_row(vec![
                        &img.id.to_string(),
                        &img.image_uuid,
                        &img.image_name,
                    ]);
                }
                println!("{table}");
            } else {
                let res = client.list_pro_private_images(page, page_size).await?;
                if ctx.out.json {
                    ctx.out.emit_json(&serde_json::json!({
                        "ok": true,
                        "kind": "pro",
                        "data": res,
                    }))?;
                    return Ok(());
                }
                if res.list.is_empty() {
                    ctx.out.println("No private images found.");
                    return Ok(());
                }
                let mut table = make_table(&["UUID", "Name", "Status", "Size", "Created"]);
                for img in res.list {
                    table.add_row(vec![
                        &img.image_uuid,
                        &img.name,
                        &img.status,
                        &format_bytes(img.image_size),
                        &img.create_at,
                    ]);
                }
                println!("{table}");
            }
        }
        ImagesCommand::Save { session, name } => {
            let resolved = resolve_session(session.as_deref(), &profile, &store, &client).await?;
            let resp = client.save_pro_image(&resolved.instance_uuid, &name).await?;
            if ctx.out.json {
                ctx.out.emit_json(&serde_json::json!({
                    "ok": true,
                    "instance_uuid": resolved.instance_uuid,
                    "image_name": name,
                    "image_uuid": resp.image_uuid,
                }))?;
            } else {
                ctx.out.println(&format!(
                    "Saving image '{}' from {} -> {}",
                    name, resolved.instance_uuid, resp.image_uuid
                ));
            }
        }
    }
    Ok(())
}

// ============================================================================
// Exclusive NFS
// ============================================================================

#[derive(Debug, Args)]
pub struct NfsArgs {
    #[command(subcommand)]
    pub command: NfsCommand,
}

#[derive(Debug, Subcommand)]
pub enum NfsCommand {
    /// Mount exclusive NFS in a data center (disables standard file storage)
    Mount {
        #[arg(long)]
        region: String,
    },
    /// Unmount exclusive NFS in a data center (switches back to standard file storage)
    Unmount {
        #[arg(long)]
        region: String,
    },
}

pub async fn handle_nfs(ctx: &CliContext, args: NfsArgs) -> Result<(), AppError> {
    let (client, _) = ctx.api_client()?;
    let (region, mount) = match args.command {
        NfsCommand::Mount { region } => (region, true),
        NfsCommand::Unmount { region } => (region, false),
    };
    client.exclusive_nfs_mount(&region, mount).await?;
    if ctx.out.json {
        ctx.out.emit_json(&serde_json::json!({
            "ok": true,
            "data_center": region,
            "exclusive_nfs_mounted": mount,
        }))?;
    } else {
        let action = if mount { "Mounted" } else { "Unmounted" };
        ctx.out
            .println(&format!("{action} exclusive NFS in region '{region}'."));
    }
    Ok(())
}

// ============================================================================
// GPU Stock
// ============================================================================

#[derive(Debug, Args)]
pub struct StockArgs {
    /// Data center region code (e.g., westDC2, beijingDC2)
    #[arg(long)]
    pub region: Option<String>,
    /// Query all known regions from the built-in catalog
    #[arg(long = "all-regions")]
    pub all_regions: bool,
    /// Filter by GPU model name(s) (e.g., "RTX 4090")
    #[arg(long)]
    pub gpu: Vec<String>,
    /// Minimum CUDA version (e.g., 11.8 or 118)
    #[arg(long)]
    pub cuda: Option<String>,
}

pub async fn handle_stock(ctx: &CliContext, args: StockArgs) -> Result<(), AppError> {
    if args.region.is_none() && !args.all_regions {
        return Err(AppError::Validation(
            "Pass --region <CODE> (e.g., westDC2) or --all-regions".into(),
        ));
    }
    let (client, _) = ctx.api_client()?;
    let cuda_v = match args.cuda.as_deref() {
        Some(c) => Some(parse_cuda_version(c)?),
        None => None,
    };

    let target_regions: Vec<String> = if args.all_regions {
        REGIONS.iter().map(|r| r.code.to_string()).collect()
    } else {
        vec![args.region.unwrap_or_default()]
    };

    let mut by_region = BTreeMap::new();
    for region in &target_regions {
        let req = GpuStockReq {
            region_sign: region.clone(),
            cuda_v_from: cuda_v,
            gpu_name_set: if args.gpu.is_empty() {
                None
            } else {
                Some(args.gpu.clone())
            },
            ..Default::default()
        };
        let items = client.gpu_stock(&req).await?;
        by_region.insert(region.clone(), items);
    }

    if ctx.out.json {
        ctx.out.emit_json(&serde_json::json!({
            "ok": true,
            "stock": by_region,
        }))?;
        return Ok(());
    }

    let mut table = make_table(&["Region", "GPU Model", "Idle", "Total"]);
    for (region, items) in by_region {
        for map in items {
            for (gpu_name, counts) in map {
                table.add_row(vec![
                    &region,
                    &gpu_name,
                    &counts.idle_gpu_num.to_string(),
                    &counts.total_gpu_num.to_string(),
                ]);
            }
        }
    }
    println!("{table}");
    Ok(())
}

// ============================================================================
// Catalog (offline reference tables)
// ============================================================================

#[derive(Debug, Args)]
pub struct CatalogArgs {
    #[command(subcommand)]
    pub command: CatalogCommand,
}

#[derive(Debug, Subcommand)]
pub enum CatalogCommand {
    /// Show built-in GPU specs and CLI aliases
    Gpus,
    /// Show built-in public base image UUIDs and aliases
    Images,
    /// Show built-in data center region codes
    Regions,
}

pub fn handle_catalog(ctx: &CliContext, args: CatalogArgs) -> Result<(), AppError> {
    match args.command {
        CatalogCommand::Gpus => {
            if ctx.out.json {
                ctx.out.emit_json(&serde_json::json!({
                    "ok": true,
                    "gpus": GPU_SPECS,
                }))?;
            } else {
                let mut table = make_table(&["Spec UUID", "Display Name", "Tier", "Aliases"]);
                for g in GPU_SPECS {
                    table.add_row(vec![
                        g.spec_uuid,
                        g.display_name,
                        g.tier,
                        &g.aliases.join(", "),
                    ]);
                }
                println!("{table}");
            }
        }
        CatalogCommand::Images => {
            if ctx.out.json {
                ctx.out.emit_json(&serde_json::json!({
                    "ok": true,
                    "images": BASE_IMAGES,
                }))?;
            } else {
                let mut table = make_table(&["Image UUID", "Framework", "CUDA", "Description", "Aliases"]);
                for img in BASE_IMAGES {
                    table.add_row(vec![
                        img.uuid,
                        img.framework,
                        &img.default_cuda.to_string(),
                        img.description,
                        &img.aliases.join(", "),
                    ]);
                }
                println!("{table}");
            }
        }
        CatalogCommand::Regions => {
            if ctx.out.json {
                ctx.out.emit_json(&serde_json::json!({
                    "ok": true,
                    "regions": REGIONS,
                }))?;
            } else {
                let mut table = make_table(&["Region Code", "Name"]);
                for r in REGIONS {
                    table.add_row(vec![r.code, r.name]);
                }
                println!("{table}");
            }
        }
    }
    Ok(())
}

// ============================================================================
// Elastic Deployments (Enterprise API)
// ============================================================================

#[derive(Debug, Args)]
pub struct DeployArgs {
    #[command(subcommand)]
    pub command: DeployCommand,
}

#[derive(Debug, Subcommand)]
pub enum DeployCommand {
    /// List elastic deployments
    List {
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        status: Option<String>,
        #[arg(long, default_value_t = 1)]
        page: u32,
        #[arg(long = "page-size", default_value_t = 20)]
        page_size: u32,
    },
    /// Create an elastic deployment (ReplicaSet, Job, or Container)
    Create(Box<CreateDeployArgs>),
    /// List containers belonging to a deployment
    Containers {
        #[arg(long = "uuid")]
        deployment_uuid: String,
        #[arg(long = "status", value_delimiter = ',')]
        status: Vec<String>,
        #[arg(long)]
        released: Option<bool>,
        #[arg(long, default_value_t = 1)]
        page: u32,
        #[arg(long = "page-size", default_value_t = 20)]
        page_size: u32,
    },
    /// List container lifecycle events
    Events {
        #[arg(long = "uuid")]
        deployment_uuid: String,
        #[arg(long = "container")]
        container_uuid: Option<String>,
        #[arg(long, default_value_t = 1)]
        page: u32,
        #[arg(long = "page-size", default_value_t = 20)]
        page_size: u32,
    },
    /// Stop a specific deployment container
    StopContainer {
        #[arg(long = "container")]
        container_uuid: String,
        #[arg(long)]
        decrease_replica: bool,
        #[arg(long)]
        no_cache: bool,
        #[arg(long)]
        cmd_before_shutdown: Option<String>,
    },
    /// Scale a ReplicaSet deployment replica count
    Scale {
        #[arg(long = "uuid")]
        deployment_uuid: String,
        #[arg(long = "replicas")]
        replica_num: u32,
    },
    /// Stop an active deployment
    Stop {
        #[arg(long = "uuid")]
        deployment_uuid: String,
    },
    /// Delete a deployment
    Delete {
        #[arg(long = "uuid")]
        deployment_uuid: String,
    },
    /// Blacklist the host machine of an unhealthy deployment container
    BlacklistAdd {
        #[arg(long = "container")]
        container_uuid: String,
        #[arg(long = "expire-minutes")]
        expire_in_minutes: Option<u32>,
        #[arg(long)]
        comment: Option<String>,
    },
    /// List active host scheduling blacklist entries
    BlacklistList,
    /// Query duration-package (DDP) balance for a deployment
    Ddp {
        #[arg(long = "uuid")]
        deployment_uuid: String,
    },
}

#[derive(Debug, Args)]
pub struct CreateDeployArgs {
    #[arg(long)]
    pub name: String,
    /// ReplicaSet, Job, or Container
    #[arg(long = "type", default_value = "Container")]
    pub deployment_type: String,
    #[arg(long)]
    pub replicas: Option<u32>,
    #[arg(long)]
    pub parallelism: Option<u32>,
    #[arg(long)]
    pub reuse_container: bool,
    #[arg(long = "region", value_delimiter = ',', required = true)]
    pub regions: Vec<String>,
    #[arg(long = "gpu", value_delimiter = ',', required = true)]
    pub gpus: Vec<String>,
    #[arg(long = "gpu-num", default_value_t = 1)]
    pub gpu_num: u32,
    #[arg(long)]
    pub image: String,
    #[arg(long = "cmd")]
    pub cmd: String,
    #[arg(long = "cmd-before-shutdown")]
    pub cmd_before_shutdown: Option<String>,
    #[arg(long = "cuda-from", default_value = "11.8")]
    pub cuda_from: String,
    #[arg(long = "cuda-to", default_value = "12.8")]
    pub cuda_to: String,
    #[arg(long = "cpu-from", default_value_t = 1)]
    pub cpu_from: u32,
    #[arg(long = "cpu-to", default_value_t = 100)]
    pub cpu_to: u32,
    #[arg(long = "mem-from", default_value_t = 1)]
    pub mem_from: u32,
    #[arg(long = "mem-to", default_value_t = 256)]
    pub mem_to: u32,
    #[arg(long = "price-from", default_value_t = 100)]
    pub price_from: u32,
    #[arg(long = "price-to", default_value_t = 10000)]
    pub price_to: u32,
}

pub async fn handle_deploy(ctx: &CliContext, args: DeployArgs) -> Result<(), AppError> {
    let (client, _) = ctx.api_client()?;

    match args.command {
        DeployCommand::List {
            name,
            status,
            page,
            page_size,
        } => {
            let res = client
                .list_deployments(&DeploymentListReq {
                    page_index: page,
                    page_size,
                    name,
                    status,
                    deployment_uuid: None,
                })
                .await?;
            if ctx.out.json {
                ctx.out.emit_json(&serde_json::json!({ "ok": true, "data": res }))?;
            } else {
                let mut table = make_table(&["UUID", "Name", "Type", "Status", "Running/Replica", "Created"]);
                for d in res.list {
                    table.add_row(vec![
                        &d.uuid,
                        &d.name,
                        &d.deployment_type,
                        &d.status,
                        &format!("{}/{}", d.running_num, d.replica_num),
                        &d.created_at,
                    ]);
                }
                println!("{table}");
            }
        }
        DeployCommand::Create(c) => {
            let cuda_v_from = parse_cuda_version(&c.cuda_from)?;
            let cuda_v_to = parse_cuda_version(&c.cuda_to)?;
            let req = CreateDeploymentReq {
                name: c.name,
                deployment_type: c.deployment_type,
                replica_num: c.replicas,
                parallelism_num: c.parallelism,
                reuse_container: Some(c.reuse_container),
                reuse_container_scope: None,
                container_template: ContainerTemplate {
                    dc_list: c.regions,
                    gpu_name_set: c.gpus,
                    gpu_num: c.gpu_num,
                    cuda_v_from,
                    cuda_v_to,
                    cpu_num_from: c.cpu_from,
                    cpu_num_to: c.cpu_to,
                    memory_size_from: c.mem_from,
                    memory_size_to: c.mem_to,
                    price_from: c.price_from,
                    price_to: c.price_to,
                    image_uuid: c.image,
                    cmd: c.cmd,
                    cmd_before_shutdown: c.cmd_before_shutdown,
                    service_6006_port_protocol: None,
                    service_6008_port_protocol: None,
                },
            };
            let resp = client.create_deployment(&req).await?;
            if ctx.out.json {
                ctx.out.emit_json(&serde_json::json!({
                    "ok": true,
                    "deployment_uuid": resp.deployment_uuid,
                }))?;
            } else {
                ctx.out
                    .println(&format!("Created deployment {}", resp.deployment_uuid));
            }
        }
        DeployCommand::Containers {
            deployment_uuid,
            status,
            released,
            page,
            page_size,
        } => {
            let res = client
                .list_deployment_containers(&DeploymentContainerListReq {
                    deployment_uuid,
                    container_uuid: None,
                    released,
                    status: if status.is_empty() {
                        None
                    } else {
                        Some(status)
                    },
                    page_index: page,
                    page_size,
                })
                .await?;
            if ctx.out.json {
                ctx.out.emit_json(&serde_json::json!({ "ok": true, "data": res }))?;
            } else {
                let mut table = make_table(&["Container UUID", "Region", "Status", "GPU", "SSH"]);
                for c in res.list {
                    let ssh = c
                        .info
                        .as_ref()
                        .map(|i| i.ssh_command.as_str())
                        .unwrap_or("-");
                    table.add_row(vec![
                        &c.uuid,
                        &c.data_center,
                        &c.status,
                        &format!("{} x{}", c.gpu_name, c.gpu_num),
                        ssh,
                    ]);
                }
                println!("{table}");
            }
        }
        DeployCommand::Events {
            deployment_uuid,
            container_uuid,
            page,
            page_size,
        } => {
            let res = client
                .list_deployment_container_events(&ContainerEventListReq {
                    deployment_uuid,
                    deployment_container_uuid: container_uuid,
                    page_index: page,
                    page_size,
                    offset: None,
                })
                .await?;
            if ctx.out.json {
                ctx.out.emit_json(&serde_json::json!({ "ok": true, "data": res }))?;
            } else {
                let mut table = make_table(&["Container UUID", "Status", "Timestamp"]);
                for ev in res.list {
                    table.add_row(vec![&ev.deployment_container_uuid, &ev.status, &ev.created_at]);
                }
                println!("{table}");
            }
        }
        DeployCommand::StopContainer {
            container_uuid,
            decrease_replica,
            no_cache,
            cmd_before_shutdown,
        } => {
            client
                .stop_deployment_container(&StopContainerReq {
                    deployment_container_uuid: container_uuid.clone(),
                    decrease_one_replica_num: Some(decrease_replica),
                    no_cache: Some(no_cache),
                    cmd_before_shutdown,
                })
                .await?;
            if ctx.out.json {
                ctx.out.emit_json(&serde_json::json!({ "ok": true, "stopped": container_uuid }))?;
            } else {
                ctx.out.println(&format!("Stopped container {container_uuid}"));
            }
        }
        DeployCommand::Scale {
            deployment_uuid,
            replica_num,
        } => {
            client
                .set_deployment_replica_num(&deployment_uuid, replica_num)
                .await?;
            if ctx.out.json {
                ctx.out.emit_json(&serde_json::json!({
                    "ok": true,
                    "deployment_uuid": deployment_uuid,
                    "replica_num": replica_num,
                }))?;
            } else {
                ctx.out.println(&format!(
                    "Scaled deployment {deployment_uuid} to {replica_num} replicas"
                ));
            }
        }
        DeployCommand::Stop { deployment_uuid } => {
            client.stop_deployment(&deployment_uuid).await?;
            if ctx.out.json {
                ctx.out.emit_json(&serde_json::json!({ "ok": true, "stopped": deployment_uuid }))?;
            } else {
                ctx.out.println(&format!("Stopped deployment {deployment_uuid}"));
            }
        }
        DeployCommand::Delete { deployment_uuid } => {
            client.delete_deployment(&deployment_uuid).await?;
            if ctx.out.json {
                ctx.out.emit_json(&serde_json::json!({ "ok": true, "deleted": deployment_uuid }))?;
            } else {
                ctx.out.println(&format!("Deleted deployment {deployment_uuid}"));
            }
        }
        DeployCommand::BlacklistAdd {
            container_uuid,
            expire_in_minutes,
            comment,
        } => {
            client
                .add_deployment_blacklist(&AddBlacklistReq {
                    deployment_container_uuid: container_uuid.clone(),
                    expire_in_minutes,
                    comment,
                })
                .await?;
            if ctx.out.json {
                ctx.out.emit_json(&serde_json::json!({ "ok": true, "blacklisted_container": container_uuid }))?;
            } else {
                ctx.out.println(&format!("Blacklisted host for container {container_uuid}"));
            }
        }
        DeployCommand::BlacklistList => {
            let items = client.list_deployment_blacklist().await?;
            if ctx.out.json {
                ctx.out.emit_json(&serde_json::json!({ "ok": true, "blacklist": items }))?;
            } else {
                let mut table = make_table(&["Machine ID", "Region", "Expires", "Message"]);
                for b in items {
                    table.add_row(vec![&b.machine_id, &b.data_center, &b.expired_time, &b.msg]);
                }
                println!("{table}");
            }
        }
        DeployCommand::Ddp { deployment_uuid } => {
            let items = client.deployment_ddp_overview(&deployment_uuid).await?;
            if ctx.out.json {
                ctx.out.emit_json(&serde_json::json!({ "ok": true, "ddp": items }))?;
            } else {
                let mut table = make_table(&["GPU Type", "Balance (s)", "Total (s)", "Regions"]);
                for item in items {
                    table.add_row(vec![
                        &item.gpu_type,
                        &item.balance.to_string(),
                        &item.total.to_string(),
                        &item.dc_list,
                    ]);
                }
                println!("{table}");
            }
        }
    }
    Ok(())
}
