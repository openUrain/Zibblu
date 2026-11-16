# Zibblu 在线服务部署

Local-First 在线服务用 PostgreSQL 17 保存账号、设备、目录、投递元数据和临时载荷。Docker 应用容器监听 `0.0.0.0:7841`，宿主机只发布环回端口。公网 HTTPS 部署须经 Owner 单独批准。开放更多注册前先完成注册和登录限流。

## Issue #46 · Win11 隔离验收

在 Docker Desktop 的 Linux Engine 上执行。测试脚本使用专用 Compose 项目，发布两个独立的环回端口，自动回收仅用于生成的测试库。先确认所选项目名、端口和资料目录专用于本次测试。默认资源如下。

| 资源 | 验收环境默认值 |
| --- | --- |
| 项目、数据库 | `zibblu-issue46`、`zibblu_issue46` |
| 应用、数据库容器 | `zibblu-issue46-zibblu-online-1`、`zibblu-issue46-postgres-1` |
| PostgreSQL 卷 | `zibblu-issue46_zibblu_pgdata` |
| 环回地址 | API `http://127.0.0.1:17841`，PostgreSQL `127.0.0.1:17846` |
| Windows 资料 | 分别设置 `ZIBBLU_DATA_DIR` 到两个新建目录 |

```powershell
docker --context desktop-linux compose ls
.\deploy\docker\Start-LocalFirstTest.ps1
curl.exe --fail http://127.0.0.1:17841/healthz

# 另起一套短 TTL 测试，使用独立数据库和数据卷。
.\deploy\docker\Start-LocalFirstTest.ps1 -ProjectName zibblu-issue46-ttl -HttpPort 17842 -DbPort 17847 -ShortTtl
```

脚本使用 Windows 上的 `desktop-linux` 和固定环回端口。同一项目重复执行会复用该项目的数据库。短 TTL 参数为文字和审批前问候 6 秒、成品文件 15 秒、未发布上传 8 秒、设备闲置 60 秒、回收间隔 1 秒。要并行开启两套环境，请使用不同项目名、端口和 Windows 资料目录。

在 Visual Studio x64 开发者终端构建客户端。分别用两个 PowerShell 进程从同一份检出启动。下面第一段在 A 终端执行；在 B 终端将 `client-a` 改为 `client-b`。在登录页的 **Advanced** 选择测试 API，创建合成账号。

```powershell
npm --prefix apps/desktop ci
npm --prefix apps/desktop run build
cargo build --locked -p zibblu-desktop --features custom-protocol

$env:ZIBBLU_DATA_DIR = Join-Path (Get-Location) '.local/issue46/client-a'
.\target\debug\zibblu-desktop.exe
```

两客户端先建立联系人关系，再测试正常消息、接收方退出及恢复、已登录发送方断网入队、关闭和重新打开设备、撤回、搜索、附件保存和暂停续传。验证历史和已保存文件的本机可用性。接收端展示文件卡片后检查元数据 ACK，实际保存并校验 SHA-256 后检查内容 ACK。短 TTL 测试让一个设备在到期前接收并保存，另一个设备在到期后同步，检查过期占位、`410` 和已保存文件仍可打开。用 Docker `exec` 从对应容器核对正文、上传和分块的在线回收。

在常规验收栈的 PostgreSQL 中另建空库，可运行同 CI 一致的 Rust 集成测试。以下使用脚本的默认合成凭据。Rust 测试将启动独立的环回 Axum 实例，并控制个别测试记录的期限及回收故障。

```powershell
docker --context desktop-linux exec zibblu-issue46-postgres-1 createdb -U zibblu zibblu_issue46_ci
$env:ZIBBLU_TEST_DATABASE_URL = 'postgres://zibblu:zibblu_issue46_fixture_only@127.0.0.1:17846/zibblu_issue46_ci'
cargo test --locked -p zibblu-online --test m1 -- --nocapture
```

设备退役或失活后，重新输入密码可登记并核对原本机记录，界面会提示旧投递可能已过期。如果设备游标与服务端不匹配，保留原资料目录，从新的 `ZIBBLU_DATA_DIR` 登录。新资料只接续仍有效的未完成投递，旧资料存在其原路径。验收结束可停止这两个指定容器以保留证据。删除旧资料、测试库或卷之前，先列清对应资源并获取 Owner 确认。

```powershell
docker --context desktop-linux stop zibblu-issue46-zibblu-online-1 zibblu-issue46-postgres-1
```

## 未来切换与回滚

- 使用新的 Zibblu PostgreSQL 库及新客户端资料。SQLx `0007_local_first` 遇到含有旧账号的库会拒绝。旧环境及数据库供回滚。旧客户端访问 v2 部署的登录或同步入口时收到升级提示，旧数据留在原环境。
- 生产切换前单独确认旧和新项目、应用/数据库容器、卷、路由、客户端资料及已核验备份。先并行启动隔离入口，再按 Owner 批准方案变更 HTTPS 路由。当前常驻 `server` 项目、Cloudflared 和旧客户端使用原服务与库。
- 回滚时将流量切回旧服务及原库，停止新应用，保留新库与新资料用于后续核验。两库各自保留其写入；将旧快照接到新入口会遗漏切换后的事务。
- 启用清理前核验备份、WAL 和恢复时点，在隔离环境先校验过期、撤回、附件摘要及幂等键。涉及备份期限和现有数据重置均须单独审批。

| 环境变量 | 默认值与作用 |
| --- | --- |
| `ZIBBLU_TEXT_TTL_SECONDS` | `259200`。新发布文字、显式共享 AI 文字以及尚未批准的问候的在线窗口 |
| `ZIBBLU_FILE_TTL_SECONDS` | 空。部署批准后指定成品文件的下载秒数 |
| `ZIBBLU_UPLOAD_TTL_SECONDS` | 空。部署批准后指定未发布上传的保留秒数 |
| `ZIBBLU_DEVICE_IDLE_SECONDS`、`ZIBBLU_DEVICE_LIMIT` | 空。经批准可指定设备闲置秒数和每账号活跃上限 |
| `ZIBBLU_CLEANUP_INTERVAL_SECONDS` | 空。默认无后台回收。经批准可指定正数间隔 |
| `ZIBBLU_HTTP_PORT` | `7841`。宿主机环回 HTTP 端口 |

`GET /healthz` 在应用与数据库可用时返回 `200`，查询异常或超时返回 `503`。应用收到 `SIGTERM` 停止接受新连接，Compose 提供 60 秒等待进行中的请求。

## 旧版同库搬迁记录

下文记录 PR #32 将旧在线服务迁入 Docker 的运维步骤，仅适用于当时的旧版。Local-First 新库使用上面的流程。

## 现有 Windows 部署的切换清单

生产操作须在 [Issue #31](https://github.com/openUrain/Zibblu/issues/31) 的 PR 审查和 Owner 批准后执行。只读审计从 `D:\Zibblu\server\compose.yaml`、Docker 数据盘中的容器元数据和 PostgreSQL 离线目录确认了以下信息。

| 项目 | 审计值 |
| --- | --- |
| 部署目录与项目名 | `D:\Zibblu\server`，`server` |
| PostgreSQL 容器及镜像 | `server-postgres-1`，`postgres:17-alpine` |
| 卷及容器内路径 | `server_zibblu_pgdata`，`/var/lib/postgresql/data` |
| 库名、所有者和网络别名 | 库与用户均为 `zibblu`，`server_default` 网络别名为 `postgres` |
| 宿主机数据库映射 | `127.0.0.1:5433:5432`，供原服务与回滚使用 |
| Docker Desktop 数据盘 | `D:\Docker\data\disk\docker_data.vhdx` |
| 应用迁移 | 生产磁盘中 SQLx 版本 `1` 至 `5` 的校验值与仓库一致 |

### 1. 核对原数据库并预构建镜像

在 Windows 宿主机的 `desktop-linux` 上确认 Docker Desktop 已使用原 D 盘数据文件。将 `$repo` 设为另一个已审查版本的检出目录，保留 `D:\Zibblu\source` 供原进程回滚。以下命令只输出指定元数据和迁移清单。继续前核对表中各项、现有容器 ID 及迁移校验值。若 Docker Engine 离线，由 Owner 在维护窗口启动 Docker Desktop。若原 PostgreSQL 容器已存在且停止，用 `docker start server-postgres-1` 启动同一容器。容器、卷、网络或迁移不符时，先查明差异。

```powershell
$repo = 'D:\Zibblu\compose-source'  # 设为已审查版本的实际路径
docker context show
docker version --format '{{.Server.Version}}'
Get-Service Cloudflared
docker inspect server-postgres-1 --format 'project={{index .Config.Labels "com.docker.compose.project"}} id={{.Id}} image={{.Config.Image}} state={{.State.Status}} ports={{json .HostConfig.PortBindings}}'
docker inspect server-postgres-1 --format '{{range .Mounts}}{{.Name}} -> {{.Destination}}{{println}}{{end}}'
docker volume inspect server_zibblu_pgdata --format '{{.Name}} {{.Driver}}'
docker network inspect server_default --format '{{range .Containers}}{{.Name}}{{println}}{{end}}'
docker exec server-postgres-1 pg_isready -h 127.0.0.1 -U zibblu -d zibblu
docker exec server-postgres-1 psql -U zibblu -d zibblu -v ON_ERROR_STOP=1 -c "SELECT current_database(), current_user, current_setting('server_version')"
docker exec server-postgres-1 psql -U zibblu -d zibblu -v ON_ERROR_STOP=1 -c "SELECT version, success, encode(checksum, 'hex') FROM _sqlx_migrations ORDER BY version"
Get-ChildItem "$repo\services\online\migrations\*.sql" | Get-FileHash -Algorithm SHA384
```

两个配置文件直接从同一份检出读取。现有部署的 `.env` 只需提供原 `POSTGRES_PASSWORD`，库名和用户名默认 `zibblu`。覆盖文件将卷和网络标记为 `external`，默认启用的服务只有应用。旧数据库由原容器的重启策略恢复，由切换前的 `pg_isready` 和 SQL 检查确认可用。可加 `--profile existing-postgres` 查看两个服务的状态。若现场名称不同，经核实后可用 `ZIBBLU_PG_NETWORK`、`ZIBBLU_PG_VOLUME` 指定实际名称。

```powershell
$envFile = 'D:\Zibblu\server\.env'
$stack = @('compose', '-p', 'server', '--env-file', $envFile,
  '-f', "$repo\deploy\docker\compose.yaml", '-f', "$repo\deploy\docker\compose.existing.yaml")
docker @stack config --quiet
if ($LASTEXITCODE) { throw '部署配置校验失败' }
$dbId = docker inspect server-postgres-1 --format '{{.Id}}'
if ($LASTEXITCODE -or -not $dbId) { throw '未找到原 PostgreSQL 容器' }
docker @stack build zibblu-online
if ($LASTEXITCODE) { throw '应用镜像构建失败' }
```

实际部署的 `config` 使用 `--quiet`。完整配置、容器环境和连接串包含密码，只供有权限的本机维护人员使用。保留原 Docker Engine 和 D 盘数据位置。对生产项目启停和更新应用时使用服务名 `zibblu-online` 和 `--no-deps`（适用于 `up`）。避免 `docker compose down`、`down -v`、卷删除或清理命令，也避免同时启动另一个 PostgreSQL 实例挂载该卷。

### 2. 备份与恢复预演

提前在 D 盘建立限制访问的 `D:\Zibblu\backups` 目录。仅授权维护账号和 `SYSTEM` 可读写，用 `Get-Acl` 核实权限后再写入。归档含正常聊天、附件、账号数据和签发密钥。用容器内 PostgreSQL 17 工具生成 custom 格式备份，使用 `docker cp` 保持二进制完整。任一步骤失败时停止操作并检查原因。

```powershell
$tag = Get-Date -Format 'yyyyMMdd-HHmmssfff'
$archive = "D:\Zibblu\backups\zibblu-$tag.dump"
$staged = "/tmp/zibblu-$tag.dump"
docker exec -u postgres server-postgres-1 sh -ec 'umask 077; pg_dump -U zibblu -d zibblu -Fc -f "$1"' sh $staged
if ($LASTEXITCODE) { throw 'pg_dump 失败' }
docker exec -u postgres server-postgres-1 pg_restore --list $staged | Out-Null
if ($LASTEXITCODE) { throw '备份目录不可读' }
docker cp "server-postgres-1:${staged}" $archive
if ($LASTEXITCODE) { throw '备份复制失败' }
Get-FileHash -Algorithm SHA256 -LiteralPath $archive
docker exec -u postgres server-postgres-1 rm -- $staged
```

切换前，将归档复制到独立的 PostgreSQL 17 恢复演练容器。该容器用 `zibblu` 角色、独立卷和新建的空 `zibblu_restore` 库，数据库端口保持未发布。复制后将文件所有者设为容器内 `postgres`，权限设为 `0600`。以该系统用户对复制后的文件执行 `pg_restore --list`，再执行 `pg_restore -U zibblu -d zibblu_restore --single-transaction /tmp/zibblu.dump`。核对迁移版本、账号、消息及附件分块数量和抽样 SHA-256，记录结果。备份是单库快照，原容器及卷继续保留原角色和凭据。灾难恢复时，保留原卷，在另一个 PostgreSQL 17 实例按已验证的步骤恢复后再决定切换目标。日常应用回滚直接使用原库。

### 3. 切换与验收

1. Owner 停止原 PowerShell 中的 `cargo run --locked -p zibblu-online`，确认 `127.0.0.1:7841` 已释放。若原应用已经停止，直接核对端口和数据库状态。切换期间只允许一个在线应用实例写入原库。
2. 停止写入后再执行一次第 2 节的备份命令，保留最终快照并核对 `pg_restore --list`。确认 `$dbId` 仍指向原容器。只启动已构建的应用。

   ```powershell
   if ((docker inspect server-postgres-1 --format '{{.Id}}') -ne $dbId) { throw 'PostgreSQL 容器 ID 不符' }
   docker @stack up -d --no-deps --no-build --pull never --wait --wait-timeout 120 zibblu-online
   if ($LASTEXITCODE) { throw '应用未就绪，请按回滚步骤处理' }
   if ((docker inspect server-postgres-1 --format '{{.Id}}') -ne $dbId) { throw 'PostgreSQL 容器 ID 不符' }
   docker @stack --profile existing-postgres ps
   curl.exe --fail http://127.0.0.1:7841/healthz
   curl.exe -sS -o NUL -w "%{http_code}\n" https://zibblu-api.openurain.com/v1/sync
   ```

3. 核对应用只发布 `127.0.0.1:7841`，PostgreSQL 仍只映射到 `127.0.0.1:5433`。从另一台 LAN 机器连接 Windows 主机的 LAN 地址，确认两个端口均无法直连。公网无凭据的同步预期返回 `401`。再用原账号通过原域名登录，确认历史消息和附件仍在，用两个跨网络客户端发送并同步普通消息，暂停并续传一个文件，核对下载 SHA-256。
4. 验收通过后，在原 `D:\Zibblu\server\compose.yaml` 旁保留旧文件的备份，再将原文件改为以下引用。`D:\Zibblu\server\.env` 继续作为凭据来源。此后可在原目录运行 Compose，无需复制服务定义。写入前将示例路径替换为 `$repo` 的实际位置，写入后执行 `docker compose config --quiet` 与 `docker compose --profile existing-postgres ps`。

   ```yaml
   name: server
   include:
     - path:
         - D:/Zibblu/compose-source/deploy/docker/compose.yaml
         - D:/Zibblu/compose-source/deploy/docker/compose.existing.yaml
   ```

### 4. 回滚与日常运维

回滚时执行 `docker @stack stop zibblu-online`，等 `7841` 释放后，在原 `D:\Zibblu\source` 用已保留的原 `DATABASE_URL` 配置和 `cargo run --locked -p zibblu-online` 恢复，仍连接 `127.0.0.1:5433` 上的原库。若原目录已经改用引用文件，恢复备份的 Compose 文件。检查公网 `401` 后再用原账号同步。保留同一数据库能保留切换期间已提交的写入。本次部署沿用原 SQLx 迁移，正常回滚无需恢复备份。

在 `D:\Zibblu\server` 查看状态用 `docker compose --profile existing-postgres ps`，查看最近日志用 `docker compose logs --tail 100 zibblu-online`。用 `docker compose stop zibblu-online` 停应用，用 `docker compose start zibblu-online` 恢复。更新时先检出已审查的应用版本，核对迁移兼容性并备份，再运行 `docker compose build zibblu-online` 和 `docker compose up -d --no-deps --no-build --pull never --wait --wait-timeout 120 zibblu-online`。新建容器的日志用 `local` 驱动，每个服务最多保留 3 个约 10 MiB 的文件。健康检查 `503` 时先检查原 PostgreSQL 的容器状态、`pg_isready` 和认证连接。环回健康但公网不可达时，检查 Cloudflared Windows 服务和既有路由的 origin。

在 Docker Desktop 的 **Settings → General** 确认 **Start Docker Desktop when you sign in**。安排一次维护窗口重启 Windows，登录后核实 Engine 启动、原 PostgreSQL 和应用恢复，以及公网 `401` 和原账号同步。`restart: unless-stopped` 由运行中的 Engine 执行，手动停止的容器需要显式启动。当前方案依赖 Windows 登录。需要无人登录开机运行时，再规划 Linux 主机或独立服务。
