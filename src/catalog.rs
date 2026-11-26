use serde::Serialize;

use crate::error::AppError;

#[derive(Debug, Clone, Serialize)]
pub struct GpuSpecEntry {
    pub spec_uuid: &'static str,
    pub display_name: &'static str,
    pub tier: &'static str,
    pub aliases: &'static [&'static str],
}

#[derive(Debug, Clone, Serialize)]
pub struct BaseImageEntry {
    pub uuid: &'static str,
    pub framework: &'static str,
    pub description: &'static str,
    pub default_cuda: i32,
    pub aliases: &'static [&'static str],
}

#[derive(Debug, Clone, Serialize)]
pub struct RegionEntry {
    pub code: &'static str,
    pub name: &'static str,
}

/// Default public base image (PyTorch 2.0, CUDA 11.8, Ubuntu 20.04, Python 3.8).
pub const DEFAULT_IMAGE_UUID: &str = "base-image-l2t43iu6uk";
/// Default GPU spec (`4090-48G` / `v-48g`).
pub const DEFAULT_GPU_SPEC: &str = "v-48g";
/// Default CUDA minimum version (`118` = CUDA 11.8).
pub const DEFAULT_CUDA_VERSION: i32 = 118;

/// Built-in GPU specs from AutoDL Container Instance Pro API documentation.
pub const GPU_SPECS: &[GpuSpecEntry] = &[
    GpuSpecEntry {
        spec_uuid: "h800",
        display_name: "H800-80G",
        tier: "通用型",
        aliases: &["h800", "h800-80g"],
    },
    GpuSpecEntry {
        spec_uuid: "v-48g",
        display_name: "4090-48G",
        tier: "通用型",
        aliases: &["4090", "rtx4090", "4090-48g", "v-48g"],
    },
    GpuSpecEntry {
        spec_uuid: "pro6000-p",
        display_name: "PRO6000-96G",
        tier: "性能型",
        aliases: &["pro6000", "pro6000-96g", "pro6000-p"],
    },
    GpuSpecEntry {
        spec_uuid: "v-32g-p",
        display_name: "4080(S)-32G",
        tier: "性能型",
        aliases: &["4080", "4080s", "4080-32g", "4080(s)-32g", "v-32g-p"],
    },
    GpuSpecEntry {
        spec_uuid: "v-48g-350w",
        display_name: "3090-48G",
        tier: "通用型",
        aliases: &["3090", "3090-48g", "v-48g-350w"],
    },
    GpuSpecEntry {
        spec_uuid: "5090-p",
        display_name: "5090-32G",
        tier: "性能型",
        aliases: &["5090", "rtx5090", "5090-32g", "5090-p"],
    },
    GpuSpecEntry {
        spec_uuid: "4090D",
        display_name: "4090D",
        tier: "通用型",
        aliases: &["4090d"],
    },
];

/// Built-in public base images from AutoDL API documentation.
pub const BASE_IMAGES: &[BaseImageEntry] = &[
    BaseImageEntry {
        uuid: "base-image-l2t43iu6uk",
        framework: "PyTorch",
        description: "cuda11.8-cudnn8-devel-ubuntu20.04-py38-torch2.0.0",
        default_cuda: 118,
        aliases: &["pytorch", "pytorch2", "pytorch2.0", "torch2", "torch2.0"],
    },
    BaseImageEntry {
        uuid: "base-image-l374uiucui",
        framework: "PyTorch",
        description: "cuda11.3-cudnn8-devel-ubuntu20.04-py38-torch1.11.0",
        default_cuda: 113,
        aliases: &["pytorch1.11", "torch1.11"],
    },
    BaseImageEntry {
        uuid: "base-image-u9r24vthlk",
        framework: "PyTorch",
        description: "cuda11.3-cudnn8-devel-ubuntu20.04-py38-torch1.10.0",
        default_cuda: 113,
        aliases: &["pytorch1.10", "torch1.10"],
    },
    BaseImageEntry {
        uuid: "base-image-12be412037",
        framework: "PyTorch",
        description: "cuda11.1-cudnn8-devel-ubuntu18.04-py38-torch1.9.0",
        default_cuda: 111,
        aliases: &["pytorch1.9", "torch1.9"],
    },
    BaseImageEntry {
        uuid: "base-image-uxeklgirir",
        framework: "TensorFlow",
        description: "cuda11.2-cudnn8-devel-ubuntu20.04-py38-tf2.9.0",
        default_cuda: 112,
        aliases: &["tf", "tf2.9", "tensorflow", "tensorflow2.9"],
    },
    BaseImageEntry {
        uuid: "base-image-0gxqmciyth",
        framework: "TensorFlow",
        description: "cuda11.2-cudnn8-devel-ubuntu18.04-py38-tf2.5.0",
        default_cuda: 112,
        aliases: &["tf2.5", "tensorflow2.5"],
    },
    BaseImageEntry {
        uuid: "base-image-4bpg0tt88l",
        framework: "TensorFlow",
        description: "cuda11.4-py38-tf1.15.5",
        default_cuda: 114,
        aliases: &["tf1", "tf1.15", "tensorflow1.15"],
    },
    BaseImageEntry {
        uuid: "base-image-mbr2n4urrc",
        framework: "Miniconda",
        description: "cuda11.6-cudnn8-devel-ubuntu20.04-py38",
        default_cuda: 116,
        aliases: &["miniconda", "conda", "conda11.6"],
    },
    BaseImageEntry {
        uuid: "base-image-7bn8iqhkb5",
        framework: "Miniconda",
        description: "cudagl11.3-cudnn8-devel-ubuntu20.04-py38",
        default_cuda: 113,
        aliases: &["cudagl", "conda11.3"],
    },
    BaseImageEntry {
        uuid: "base-image-h041hn36yt",
        framework: "Miniconda",
        description: "cuda11.1-cudnn8-devel-ubuntu18.04-py38",
        default_cuda: 111,
        aliases: &["conda11.1"],
    },
    BaseImageEntry {
        uuid: "base-image-qkkhitpik5",
        framework: "Miniconda",
        description: "cuda10.2-cudnn7-devel-ubuntu18.04-py38",
        default_cuda: 102,
        aliases: &["conda10.2"],
    },
    BaseImageEntry {
        uuid: "base-image-k0vep6kyq8",
        framework: "Miniconda",
        description: "cuda9.0-cudnn7-devel-ubuntu16.04-py36",
        default_cuda: 90,
        aliases: &["conda9.0"],
    },
    BaseImageEntry {
        uuid: "base-image-l2843iu23k",
        framework: "TensorRT",
        description: "cuda11.8-cudnn8-devel-ubuntu20.04-py38-trt8.5.1",
        default_cuda: 118,
        aliases: &["tensorrt", "trt", "trt8.5"],
    },
];

/// Built-in AutoDL data center region codes.
pub const REGIONS: &[RegionEntry] = &[
    RegionEntry {
        code: "westDC2",
        name: "西北企业区(推荐)",
    },
    RegionEntry {
        code: "westDC3",
        name: "西北B区",
    },
    RegionEntry {
        code: "beijingDC1",
        name: "北京A区",
    },
    RegionEntry {
        code: "beijingDC2",
        name: "北京B区",
    },
    RegionEntry {
        code: "beijingDC4",
        name: "L20专区(原北京C区)",
    },
    RegionEntry {
        code: "beijingDC3",
        name: "V100专区(原华南A区)",
    },
    RegionEntry {
        code: "neimengDC1",
        name: "内蒙A区",
    },
    RegionEntry {
        code: "neimengDC3",
        name: "内蒙B区",
    },
    RegionEntry {
        code: "foshanDC1",
        name: "佛山区",
    },
    RegionEntry {
        code: "chongqingDC1",
        name: "重庆A区",
    },
    RegionEntry {
        code: "yangzhouDC1",
        name: "3090专区",
    },
];

/// Resolve a friendly GPU name (e.g., `4090`, `H800`, `4090d`) or raw `gpu_spec_uuid`.
/// Returns `(resolved_spec_uuid, is_known_builtin)`.
pub fn resolve_gpu_spec(input: &str) -> (String, bool) {
    let trimmed = input.trim();
    let lower = trimmed.to_ascii_lowercase();
    for entry in GPU_SPECS {
        if entry.spec_uuid.eq_ignore_ascii_case(trimmed)
            || entry.display_name.eq_ignore_ascii_case(trimmed)
            || entry.aliases.iter().any(|a| *a == lower)
        {
            return (entry.spec_uuid.to_string(), true);
        }
    }
    (trimmed.to_string(), false)
}

/// Resolve a friendly image alias (`pytorch2.0`, `miniconda`, etc.) or raw UUID (`image-...` / `base-image-...`).
/// Returns `(resolved_image_uuid, suggested_cuda_version)`.
pub fn resolve_image_uuid(input: &str) -> (String, Option<i32>) {
    let trimmed = input.trim();
    let lower = trimmed.to_ascii_lowercase();
    for entry in BASE_IMAGES {
        if entry.uuid.eq_ignore_ascii_case(trimmed) || entry.aliases.iter().any(|a| *a == lower) {
            return (entry.uuid.to_string(), Some(entry.default_cuda));
        }
    }
    (trimmed.to_string(), None)
}

/// Parse CUDA version strings like `"11.8"`, `"12.1"`, `"118"`, `"121"` into AutoDL integer format.
pub fn parse_cuda_version(input: &str) -> Result<i32, AppError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(AppError::Validation("CUDA version cannot be empty".into()));
    }
    if let Some((major_s, minor_s)) = trimmed.split_once('.') {
        let major: i32 = major_s.parse().map_err(|_| {
            AppError::Validation(format!("Invalid CUDA major version in '{trimmed}'"))
        })?;
        let minor: i32 = minor_s.parse().map_err(|_| {
            AppError::Validation(format!("Invalid CUDA minor version in '{trimmed}'"))
        })?;
        if !(0..=9).contains(&minor) || major < 8 {
            return Err(AppError::Validation(format!(
                "Invalid CUDA version '{trimmed}' (expected e.g. 11.8 or 12.1)"
            )));
        }
        return Ok(major * 10 + minor);
    }
    let val: i32 = trimmed.parse().map_err(|_| {
        AppError::Validation(format!(
            "Invalid CUDA version '{trimmed}' (expected e.g. 11.8 or 118)"
        ))
    })?;
    if val < 80 {
        return Err(AppError::Validation(format!(
            "Invalid CUDA integer '{val}' (expected e.g. 118 for 11.8)"
        )));
    }
    Ok(val)
}
