use crate::reconcile;
use crate::session_manager::{ManagerError, SessionManager};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use latch_core::config::{GlobalConfig, Paths};
use latch_core::model::SessionStatus;
use latch_core::protocol::{
    FRAME_PTY_DROPPED, FRAME_PTY_OUTPUT, FRAME_PTY_SNAPSHOT, FRAME_RPC, MAX_RPC_FRAME_BYTES,
    RpcNotification, RpcRequest, RpcResponse, encode_dropped, err_code,
};
use latch_git::{DiffOptions, FileChangeKind, MergeCheckResult, MergeStrategy};
use latch_store::Store;
use serde::Deserialize;
use serde_json::{Value, json};
use std::fs;
use std::future::Future;
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Notify, broadcast};

#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub paths: Paths,
    pub global: GlobalConfig,
}

struct ServerState {
    manager: Arc<SessionManager>,
    store: Store,
    orphans: Vec<String>,
    shutdown_notify: Arc<Notify>,
}

/// Menjalankan server UDS JSON-RPC Latch sampai sinyal `shutdown` atau `daemon.shutdown`.
pub async fn run(cfg: DaemonConfig, shutdown: impl Future<Output = ()>) -> anyhow::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&cfg.paths.state_dir)?;
    if let Some(parent) = cfg.paths.socket_path.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }

    if cfg.paths.socket_path.exists() {
        if StdUnixStream::connect(&cfg.paths.socket_path).is_ok() {
            anyhow::bail!(
                "daemon already running at {}",
                cfg.paths.socket_path.display()
            );
        }
        fs::remove_file(&cfg.paths.socket_path)?;
    }

    let store = Store::open(&cfg.paths.db_path())?;
    let orphans = reconcile::run(&store, &cfg.paths).await?;

    let sandbox: Arc<dyn latch_core::SandboxProvider> =
        Arc::from(latch_sandbox::default_provider());
    let manager = Arc::new(SessionManager::new(
        store.clone(),
        cfg.paths.clone(),
        cfg.global.clone(),
        sandbox,
    ));

    let listener = UnixListener::bind(&cfg.paths.socket_path)?;
    fs::set_permissions(&cfg.paths.socket_path, fs::Permissions::from_mode(0o600))?;
    tracing::info!(socket = %cfg.paths.socket_path.display(), "latch daemon listening");

    let shutdown_notify = Arc::new(Notify::new());
    let state = Arc::new(ServerState {
        manager: Arc::clone(&manager),
        store,
        orphans,
        shutdown_notify: Arc::clone(&shutdown_notify),
    });

    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                tracing::info!("shutdown signal received");
                break;
            }
            _ = shutdown_notify.notified() => {
                tracing::info!("shutdown requested via RPC");
                break;
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        let st = Arc::clone(&state);
                        tokio::spawn(async move {
                            if let Err(err) = handle_connection(stream, st).await {
                                tracing::debug!(error = %err, "connection closed");
                            }
                        });
                    }
                    Err(err) => {
                        tracing::error!(error = %err, "accept error");
                    }
                }
            }
        }
    }

    manager.shutdown_all().await;
    let _ = fs::remove_file(&cfg.paths.socket_path);
    Ok(())
}

async fn handle_connection(mut stream: UnixStream, state: Arc<ServerState>) -> anyhow::Result<()> {
    let cred = stream.peer_cred()?;
    let my_uid = nix::unistd::getuid().as_raw();
    if cred.uid() != my_uid {
        let msg = format!("uid {} rejected (expected {my_uid})", cred.uid());
        let _ = state
            .store
            .call(move |db| db.audit_insert(None, "auth_failure", &msg))
            .await;
        anyhow::bail!("unauthorized peer uid");
    }

    loop {
        let Some((kind, payload)) = read_async_frame(&mut stream).await? else {
            return Ok(());
        };
        if kind != FRAME_RPC {
            let resp = RpcResponse::err(
                Value::Null,
                err_code::INVALID_REQUEST,
                "expected JSON-RPC frame",
                None,
            );
            write_rpc(&mut stream, &resp).await?;
            continue;
        }

        let req: RpcRequest = match serde_json::from_slice(&payload) {
            Ok(r) => r,
            Err(err) => {
                let resp = RpcResponse::err(
                    Value::Null,
                    err_code::INVALID_REQUEST,
                    format!("invalid JSON-RPC: {err}"),
                    None,
                );
                write_rpc(&mut stream, &resp).await?;
                continue;
            }
        };

        if req.method == "session.attach" {
            handle_attach(&mut stream, &state, &req).await?;
            return Ok(());
        }
        if req.method == "events.subscribe" {
            handle_events(&mut stream, &state, &req).await?;
            return Ok(());
        }

        let resp = match dispatch(&state, &req.method, req.params).await {
            Ok(val) => RpcResponse::ok(req.id, val),
            Err(ManagerError::Rpc {
                code,
                message,
                details,
            }) => RpcResponse::err(req.id, code, message, details),
        };
        write_rpc(&mut stream, &resp).await?;
    }
}

async fn handle_attach(
    stream: &mut UnixStream,
    state: &ServerState,
    req: &RpcRequest,
) -> anyhow::Result<()> {
    #[derive(Deserialize)]
    struct Params {
        id: String,
    }
    let params: Params = match serde_json::from_value(req.params.clone()) {
        Ok(p) => p,
        Err(e) => {
            let resp = RpcResponse::err(
                req.id.clone(),
                err_code::INVALID_REQUEST,
                e.to_string(),
                None,
            );
            write_rpc(stream, &resp).await?;
            return Ok(());
        }
    };

    let (sid, pty) = match state.manager.session_attach_handle(&params.id).await {
        Ok(pair) => pair,
        Err(ManagerError::Rpc {
            code,
            message,
            details,
        }) => {
            let resp = RpcResponse::err(req.id.clone(), code, message, details);
            write_rpc(stream, &resp).await?;
            return Ok(());
        }
    };

    let (snapshot, mut rx) = match pty.subscribe() {
        Ok(sub) => sub,
        Err(e) => {
            let resp = RpcResponse::err(req.id.clone(), err_code::INTERNAL, e.to_string(), None);
            write_rpc(stream, &resp).await?;
            return Ok(());
        }
    };
    drop(pty);

    write_rpc(
        stream,
        &RpcResponse::ok(req.id.clone(), json!({ "session_id": sid.as_str() })),
    )
    .await?;
    write_async_frame(stream, FRAME_PTY_SNAPSHOT, &snapshot).await?;

    loop {
        match rx.recv().await {
            Ok(chunk) => {
                if !chunk.is_empty() {
                    write_async_frame(stream, FRAME_PTY_OUTPUT, &chunk).await?;
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                let marker = encode_dropped(n);
                write_async_frame(stream, FRAME_PTY_DROPPED, &marker).await?;
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
    Ok(())
}

async fn handle_events(
    stream: &mut UnixStream,
    state: &ServerState,
    req: &RpcRequest,
) -> anyhow::Result<()> {
    let mut rx = state.manager.subscribe_events();
    write_rpc(
        stream,
        &RpcResponse::ok(req.id.clone(), json!({ "subscribed": true })),
    )
    .await?;
    loop {
        match rx.recv().await {
            Ok(ev) => {
                let notif = RpcNotification {
                    jsonrpc: "2.0".to_string(),
                    method: "event".to_string(),
                    params: serde_json::to_value(&ev)?,
                };
                let bytes = serde_json::to_vec(&notif)?;
                write_async_frame(stream, FRAME_RPC, &bytes).await?;
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
    Ok(())
}

async fn dispatch(state: &ServerState, method: &str, params: Value) -> Result<Value, ManagerError> {
    match method {
        "daemon.status" => Ok(json!({
            "version": env!("CARGO_PKG_VERSION"),
            "pid": std::process::id(),
            "live_sessions": state.manager.live_count(),
            "sandbox": state.manager.sandbox_name(),
            "orphan_worktrees": state.orphans,
        })),
        "daemon.shutdown" => {
            state.shutdown_notify.notify_one();
            Ok(json!({ "shutting_down": true }))
        }
        "project.add" => {
            #[derive(Deserialize)]
            struct P {
                name: String,
                repo_path: String,
                default_branch: Option<String>,
            }
            let p: P = parse_params(params)?;
            let proj = state
                .manager
                .project_add(p.name, p.repo_path, p.default_branch)
                .await?;
            Ok(serde_json::to_value(proj).map_err(|e| ManagerError::Rpc {
                code: err_code::INTERNAL,
                message: e.to_string(),
                details: None,
            })?)
        }
        "project.list" => {
            let list = state.manager.project_list().await?;
            to_val(list)
        }
        "project.remove" => {
            #[derive(Deserialize)]
            struct P {
                name: String,
            }
            let p: P = parse_params(params)?;
            state.manager.project_remove(&p.name).await?;
            Ok(json!({ "removed": p.name }))
        }
        "session.create" => {
            #[derive(Deserialize)]
            struct P {
                project: String,
                profile: String,
                task: Option<String>,
                base_ref: Option<String>,
            }
            let p: P = parse_params(params)?;
            let sess = state
                .manager
                .session_create(&p.project, &p.profile, p.task, p.base_ref)
                .await?;
            to_val(sess)
        }
        "session.list" => {
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct P {
                project: Option<String>,
                status: Option<SessionStatus>,
            }
            let p: P = parse_params(params)?;
            let list = state
                .manager
                .session_list(p.project.as_deref(), p.status)
                .await?;
            to_val(list)
        }
        "session.get" => {
            #[derive(Deserialize)]
            struct P {
                id: String,
            }
            let p: P = parse_params(params)?;
            to_val(state.manager.resolve_session(&p.id).await?)
        }
        "session.pause" => {
            #[derive(Deserialize)]
            struct P {
                id: String,
            }
            let p: P = parse_params(params)?;
            to_val(state.manager.session_pause(&p.id).await?)
        }
        "session.resume" => {
            #[derive(Deserialize)]
            struct P {
                id: String,
            }
            let p: P = parse_params(params)?;
            to_val(state.manager.session_resume(&p.id).await?)
        }
        "session.kill" => {
            #[derive(Deserialize)]
            struct P {
                id: String,
            }
            let p: P = parse_params(params)?;
            to_val(state.manager.session_kill(&p.id).await?)
        }
        "session.input" => {
            #[derive(Deserialize)]
            struct P {
                id: String,
                data_base64: String,
            }
            let p: P = parse_params(params)?;
            let bytes = BASE64
                .decode(&p.data_base64)
                .map_err(|e| ManagerError::Rpc {
                    code: err_code::INVALID_REQUEST,
                    message: format!("invalid base64: {e}"),
                    details: None,
                })?;
            state.manager.session_input(&p.id, &bytes).await?;
            Ok(json!({ "ok": true }))
        }
        "session.resize" => {
            #[derive(Deserialize)]
            struct P {
                id: String,
                cols: u16,
                rows: u16,
            }
            let p: P = parse_params(params)?;
            state.manager.session_resize(&p.id, p.cols, p.rows).await?;
            Ok(json!({ "ok": true }))
        }
        "session.screen" => {
            #[derive(Deserialize)]
            struct P {
                id: String,
            }
            let p: P = parse_params(params)?;
            let text = state.manager.session_screen(&p.id).await?;
            Ok(json!({ "screen": text }))
        }
        "session.diff" => {
            #[derive(Deserialize)]
            struct P {
                id: String,
                file: Option<String>,
                #[serde(default)]
                offset: usize,
                #[serde(default)]
                limit: usize,
            }
            let p: P = parse_params(params)?;
            let page = state
                .manager
                .session_diff(
                    &p.id,
                    DiffOptions {
                        file: p.file,
                        offset: p.offset,
                        limit: p.limit,
                        max_bytes: 256 * 1024,
                    },
                )
                .await?;
            let files: Vec<Value> = page
                .files
                .iter()
                .map(|f| {
                    let kind = match f.kind {
                        FileChangeKind::Added => "A",
                        FileChangeKind::Modified => "M",
                        FileChangeKind::Deleted => "D",
                        FileChangeKind::Renamed => "R",
                    };
                    json!({ "kind": kind, "path": f.path, "old_path": f.old_path })
                })
                .collect();
            Ok(json!({
                "files": files,
                "patch": page.patch,
                "truncated": page.truncated,
                "total": page.total,
            }))
        }
        "session.merge_check" => {
            #[derive(Deserialize)]
            struct P {
                id: String,
            }
            let p: P = parse_params(params)?;
            match state.manager.session_merge_check(&p.id).await? {
                MergeCheckResult::Clean { tree } => Ok(json!({
                    "clean": true,
                    "tree": tree,
                    "conflicts": [],
                })),
                MergeCheckResult::Conflicts(conflicts) => Ok(json!({
                    "clean": false,
                    "conflicts": conflicts,
                })),
            }
        }
        "session.merge" => {
            #[derive(Deserialize)]
            struct P {
                id: String,
                #[serde(default = "default_strategy")]
                strategy: String,
                message: Option<String>,
            }
            fn default_strategy() -> String {
                "squash".to_string()
            }
            let p: P = parse_params(params)?;
            let strategy = match p.strategy.as_str() {
                "squash" => MergeStrategy::Squash,
                "merge" => MergeStrategy::Merge,
                other => {
                    return Err(ManagerError::Rpc {
                        code: err_code::INVALID_REQUEST,
                        message: format!("invalid merge strategy: {other}"),
                        details: None,
                    });
                }
            };
            let oid = state
                .manager
                .session_merge(&p.id, strategy, p.message)
                .await?;
            Ok(json!({ "commit": oid }))
        }
        "session.discard" => {
            #[derive(Deserialize)]
            struct P {
                id: String,
            }
            let p: P = parse_params(params)?;
            state.manager.session_discard(&p.id).await?;
            Ok(json!({ "discarded": true }))
        }
        other => Err(ManagerError::Rpc {
            code: err_code::INVALID_REQUEST,
            message: format!("unknown method: {other}"),
            details: None,
        }),
    }
}

fn parse_params<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, ManagerError> {
    let normalized = if params.is_null() { json!({}) } else { params };
    serde_json::from_value(normalized).map_err(|e| ManagerError::Rpc {
        code: err_code::INVALID_REQUEST,
        message: format!("invalid params: {e}"),
        details: None,
    })
}

fn to_val<T: serde::Serialize>(val: T) -> Result<Value, ManagerError> {
    serde_json::to_value(val).map_err(|e| ManagerError::Rpc {
        code: err_code::INTERNAL,
        message: e.to_string(),
        details: None,
    })
}

async fn write_rpc(stream: &mut UnixStream, resp: &RpcResponse) -> io::Result<()> {
    let bytes = serde_json::to_vec(resp).map_err(io::Error::other)?;
    write_async_frame(stream, FRAME_RPC, &bytes).await
}

async fn write_async_frame(stream: &mut UnixStream, kind: u8, data: &[u8]) -> io::Result<()> {
    let total_len = data.len().saturating_add(1);
    let len_u32 = u32::try_from(total_len)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame too large"))?;
    stream.write_all(&len_u32.to_be_bytes()).await?;
    stream.write_all(&[kind]).await?;
    stream.write_all(data).await?;
    stream.flush().await
}

async fn read_async_frame(stream: &mut UnixStream) -> io::Result<Option<(u8, Vec<u8>)>> {
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let total_len = u32::from_be_bytes(len_buf) as usize;
    if total_len == 0 || total_len > MAX_RPC_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid frame size",
        ));
    }
    let mut buf = vec![0u8; total_len];
    stream.read_exact(&mut buf).await?;
    let kind = buf[0];
    let data = buf.split_off(1);
    Ok(Some((kind, data)))
}
