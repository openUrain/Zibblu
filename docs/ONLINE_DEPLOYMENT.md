# Zibblu 在线服务部署

Docker Compose 使用 PostgreSQL 17 保存普通消息、附件分块、账号和 LAN 签发密钥。应用容器使用 `ZIBBLU_CONTAINER=1` 显式监听 `0.0.0.0:7841`，Docker 只将端口发布到宿主机 `127.0.0.1:7841`。公网 HTTPS 继续由现有 Cloudflared Windows 服务接入 `http://127.0.0.1:7841`，客户端继续使用 `https://zibblu-api.openurain.com`。

**扩大公开注册前仍须完成注册和登录限流。** 使用 Docker 部署后，公网入口的访问策略保持原样。

## 新部署或隔离测试

使用 Linux 容器、Docker Engine 28 或更新版本以及 Docker Compose 2.24.4 或更新版本。在仓库根目录创建本地 `.env`，并通过编辑器填入专用数据库密码。

```powershell
Copy-Item deploy/docker/.env.example .env
$stack = @('compose', '-f', 'deploy/docker/compose.yaml')
docker @stack config --quiet
docker @stack up -d --build --wait --wait-timeout 120
docker @stack ps
```

此配置为新部署创建项目专用网络和 PostgreSQL 卷。PostgreSQL 只接入内部网络，无宿主机端口。应用还接入独立 HTTP 网络以发布环回端口。已有部署使用下节的接入流程。

`GET /healthz` 在应用和数据库可用时返回 `200`，数据库连接或查询失败时返回 `503`，响应正文为空。应用尚未监听时，请求无法连接。Compose 用该接口判断就绪，用 `pg_isready` 等待 PostgreSQL 开始接受连接。`unless-stopped` 在进程退出或 Engine 重启时恢复容器，单独的 `unhealthy` 状态用于诊断。应用收到 `SIGTERM` 后停止接受新连接并等待已接收的请求，Compose 提供 60 秒。已提交的附件分块和偏移保存在同一个 PostgreSQL 事务中，客户端可在恢复后按原传输 ID 续传。

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
