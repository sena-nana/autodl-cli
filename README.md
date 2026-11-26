# autodl-cli

一个使用 **Rust** 开发的、参考 [`google-colab-cli`](https://github.com/googlecolab/google-colab-cli) 接口风格打造的 **AutoDL CLI**，专为本地终端开发与 **AI Agent 自动化调用** 设计。

## 核心特性

- **系统凭据管理器安全存储 Token**：默认通过 `keyring` 将开发者 Token 安全写入 **Windows 凭据管理器 / macOS Keychain / Linux Secret Service**，支持多 Profile 切换（`auth login` / `auth switch` / `auth list` / `auth status`），并完善了 **`logout` 账号退出**（同步清除系统凭据库密钥、`profiles.json` 元数据以及本地 `sessions.json` 会话缓存，若实例仍在运行会自动告警提示）。
- **参考 `colab-cli` 的零心智负担体验**：
  - `autodl new` / `sessions` / `status` / `exec` / `run` / `ssh` / `install` / `upload` / `download` / `ls` / `rm` / `stop` / `usage` / `whoami` / `logout`
  - 单会话自动推断：当只有一个活跃会话或运行中实例时，可省略 `-s/--session`。
- **AI Agent 优先设计**：
  - 全局 `--json`（或 `AUTODL_JSON=1`）输出结构化 JSON 到 `stdout`，所有日志、进度与警告仅输出到 `stderr`。
  - 确定性退出码：`0` 成功，`1` 一般/API 错误，`2` 参数校验错误，`3` 鉴权失败，`4` 会话未找到或存在歧义，`124` 超时，或透传远端退出码。
- **开机自执行命令包装（Boot Auto-Exec）**：
  - `new` 与 `power-on` 支持 `--start-cmd` 与 `--start-script <local.py|local.sh>`（自动 Base64 打包解码执行），默认记录日志至 `/var/log/autodl-start.log` 并将退出码写入 `/tmp/autodl_start.exit`（可通过 `autodl boot-log` 查看）。
  - 支持 `--auto-shutdown-on-exit`：开机任务完成后自动关机，适合无本地守护的批处理任务。
- **自动释放工作流（`autodl run`）**：
  - 一键编排：`创建实例 -> 等待运行 -> 连接 SSH -> 同步项目代码 (--sync) -> 上传脚本 -> 安装依赖 (uv/pip) -> 执行并流式返回输出 -> 拉取产物 (--download) -> 自动关机并释放实例`。
  - 无论任务成功、报错还是 `Ctrl+C` 中断，默认都会可靠执行关机释放（`power_off` -> `release`），避免忘记释放导致持续计费；支持 `--keep` 保留实例或 `--power-off` 仅关机。
- **纯 Rust SSH / SFTP 与极速代码同步（`autodl sync`）**：
  - 基于 `russh` + `russh-sftp` 原生实现（跨平台支持 Windows / macOS / Linux，无需本地 `sshpass` 或 `rsync`）。
  - `autodl sync` 自动遵循 `.gitignore`、`.autodlignore` 以及常见研发/ML 默认忽略项（`.git`、`target`、`node_modules`、`__pycache__`、`.venv` 等），以 `.tar.gz` 流式传输并远端解压。
- **完整覆盖 AutoDL 官方 OpenAPI**：
  - 通用接口：账户余额 `usage`、专用 NFS 挂载 `nfs mount/unmount`
  - 容器实例 Pro API：`new`、`sessions`、`status`、`power-on`、`power-off`、`stop`/`release`、`images list/save`
  - 弹性部署 & 库存 API：`stock`、`deploy` 系列子命令

---

## 安装与构建

```bash
cargo build --release
# 或安装到 PATH (~/.cargo/bin)
cargo install --path .
```

---

## 快速开始

### 1. 登录与凭据管理

```bash
# 交互式隐藏输入（Token 存入 Windows 凭据管理器 / macOS Keychain / Linux Secret Service）
autodl auth login

# Agent / CI 推荐通过 stdin 传入，避免进入 shell 历史
echo "$YOUR_TOKEN" | autodl auth login --token-stdin

# 查看当前认证状态与账户余额
autodl auth status
autodl whoami
autodl usage

# 多账号 Profile 切换
autodl --profile work auth login --token-stdin
autodl auth list
autodl auth switch default

# 完整退出当前账号（清除系统凭据 + 本地会话缓存）
autodl logout
# 退出全部账号
autodl auth logout --all
```

### 2. 一键运行并自动释放（`autodl run`）

```bash
# 同步当前项目代码，在 4090 上安装依赖并运行 train.py，拉取产物后自动释放实例
autodl run --gpu 4090 --sync . --requirements requirements.txt \
  --download /root/autodl-tmp/workspace/model.pt:./out/model.pt \
  train.py -- --epochs 10

# 以 JSON 模式供 AI Agent 解析
autodl --json run --gpu 4090 --cmd "nvidia-smi && python3 -c 'import torch; print(torch.cuda.is_available())'"
```

### 3. 会话式交互开发（类 `colab-cli` 体验）

```bash
# 查看内置 GPU 规格、基础镜像与地区代码
autodl catalog gpus
autodl catalog images
autodl catalog regions

# 创建带开机自执行脚本的实例（任务结束自动关机）
autodl new -s trainer --gpu 4090 --image pytorch2.0 --start-script ./bootstrap.sh

# 同步代码 & 执行命令（单会话时可省略 -s）
autodl sync . /root/autodl-tmp/workspace
autodl exec -f train.py -- --lr 1e-4
echo 'import torch; print(torch.cuda.device_count())' | autodl exec --python

# 查看 JupyterLab / 6006 / 6008 地址、开机日志、SSH
autodl url
autodl boot-log
autodl ssh -- nvidia-smi

# 下载产物并释放实例
autodl download /root/autodl-tmp/workspace/ckpt.bin ./ckpt.bin
autodl stop -s trainer --yes
```

### 4. 存储管理（`autodl fs` & `autodl disk` & `autodl nfs`）

```bash
# 查看实例内各挂载点（/、/root/autodl-tmp、/root/autodl-fs）容量与 inode 统计
autodl disk --du /root/autodl-tmp

# 检查共享文件存储 /root/autodl-fs 是否已挂载及 20w inode 上限占用情况
autodl fs status

# 直接面向 /root/autodl-fs 进行文件同步、浏览、目录大小统计
autodl fs sync ./dataset datasets/my-data
autodl fs ls datasets
autodl fs du --depth 2

# 预览并清理网页/Jupyter 上传中断产生的隐藏缓存
autodl fs clean-cache --dry-run
autodl fs clean-cache --yes

# 切换地区专用 NFS 挂载
autodl nfs mount --region westDC2
```

---

## 开发与测试

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```
