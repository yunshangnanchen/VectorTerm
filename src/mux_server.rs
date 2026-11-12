//! mux 守护进程 —— wezterm 式「机房」:真正持有 ConPTY / shell 子进程。
//!
//! 由 `vectorterm --mux-server` 启动,常驻后台。GUI 前端作为客户端连上它:
//! * **会话保持**:关掉所有 GUI 窗口后,这里的 shell 和它正在跑的程序
//!   继续运行,输出持续写进本进程的内存环形缓冲,不丢。
//! * **detach / attach**:客户端断开只是取消订阅,不杀进程;新客户端
//!   attach 时把该 pane 的缓冲历史一次性回放,滚动缓冲完整恢复现场。
//! * **多路复用**:同一 pane 可被多个客户端同时 attach(镜像视图)。
//!
//! 与 wezterm 的差异:wezterm 在 server 端存「解析后的稳定行」,前端拉
//! 脏行 delta;这里在 server 端存「原始 PTY 输出字节」,前端 replay 字节、
//! 用自己的 `Terminal` 重建屏幕。因为终端状态机是确定性的,两者产出的
//! 屏幕完全一致,而实现简单得多 —— 不需要跨进程序列化 CellAttributes。
//!
//! 设计上 server 端**不解析、不渲染**,只管:起 shell、转发字节、缓存字节。

use crate::mux::{self, Frame, FrameReader, PaneMeta, Msg, PIPE_NAME, PROTOCOL_VERSION};
use crate::pty;
use anyhow::{bail, Context};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// detach 期间每个 pane 缓存多少字节的输出(约 16MB)。
/// 超出后从最旧开始丢,保证内存有界 —— 和终端滚动缓冲的行数上限是同一个道理。
const OUTPUT_BUFFER_LIMIT: usize = 16 * 1024 * 1024;

/// 一个 pane 的完整状态。
struct Pane {
    id: u64,
    /// ConPTY 会话(shell 子进程 + killer)。
    session: pty::PtySession,
    /// 写端:向 shell 发键盘输入。
    writer: Box<dyn Write + Send>,
    title: String,
    pid: u32,
    /// detach 期间的输出缓存(原始字节)。attach 时回放。
    buffer: Vec<u8>,
    /// 当前 attach 的客户端(pane id → 写端)。多个客户端可镜像。
    clients: HashMap<u64, Arc<Mutex<Box<dyn Write + Send>>>>,
    exited: bool,
}

/// 守护进程全局状态。
struct Server {
    panes: Mutex<HashMap<u64, Arc<Mutex<Pane>>>>,
    next_pane: AtomicU64,
    next_client: AtomicU64,
}

impl Server {
    fn new() -> Self {
        Self {
            panes: Mutex::new(HashMap::new()),
            next_pane: AtomicU64::new(1),
            next_client: AtomicU64::new(1),
        }
    }
}

/// 起一个 pane 的 shell。几何尺寸、shell、参数、环境都以客户端请求为准。
fn spawn_pty(req: &mux::SpawnReq) -> anyhow::Result<pty::PtyParts> {
    pty::spawn_with(
        &req.shell,
        &req.args,
        req.cwd.as_deref(),
        &req.env,
        req.rows,
        req.cols,
        req.pixel_w,
        req.pixel_h,
    )
    .with_context(|| format!("起 shell 失败({})", req.shell))
}

/// 守护进程入口。阻塞直到进程被终止。`_cfg` 留作将来扩展(默认 shell 等)。
pub fn run(_cfg: &crate::config::AppConfig) -> anyhow::Result<()> {
    // 已经是守护进程?直接再起一个会抢管道。先探测:能连上说明已有人在跑。
    if mux::MuxClient::connect()?.is_some() {
        crate::wlog!("[VectorTerm] mux 守护进程已在运行,本实例退出");
        return Ok(());
    }

    let server = Arc::new(Server::new());
    let mut listener = listen_pipe()?;
    crate::wlog!("[VectorTerm] mux 守护进程已就绪,监听 {}", PIPE_NAME);

    // 单线程 accept 循环:每个客户端开一条线程处理。
    loop {
        let stream = listener.accept()?;
        let server = server.clone();
        let _ = std::thread::Builder::new()
            .name("vectorterm-mux-client".into())
            .spawn(move || {
                if let Err(e) = handle_client(server, stream) {
                    crate::werr!("[VectorTerm] 客户端处理失败:{e:#}");
                }
            });
    }
}

/// 处理一条客户端连接:握手 → 读请求 → 路由。
fn handle_client(server: Arc<Server>, stream: std::fs::File) -> anyhow::Result<()> {
    let client_id = server.next_client.fetch_add(1, Ordering::Relaxed);
    // 读、写各一个独立句柄
    let reader_stream = stream.try_clone().context("clone 管道读端")?;
    let mut reader = FrameReader::new(reader_stream);
    let mut writer = stream;

    // 握手
    let Some(frame) = reader.read_frame()? else {
        return Ok(());
    };
    if frame.kind != Msg::Hello {
        bail!("首条消息不是 Hello");
    }
    let ver = u32::from_le_bytes(
        frame
            .payload
            .as_slice()
            .try_into()
            .context("Hello payload 长度错误")?,
    );
    if ver != PROTOCOL_VERSION {
        mux::write_frame(&mut writer, Msg::Error, "协议版本不一致".as_bytes())?;
        return Ok(());
    }
    mux::write_frame(&mut writer, Msg::Hello, &PROTOCOL_VERSION.to_le_bytes())?;

    loop {
        let Some(frame) = reader.read_frame()? else {
            break; // 客户端断开
        };
        // 处理请求。kill 之后 pane 可能已移除,后续消息继续处理其余 pane。
        match dispatch(&server, client_id, &mut writer, &frame) {
            Ok(()) => {}
            Err(e) => {
                // 单条消息失败不整断连接,回一条 Error(除推送外)。
                crate::wlog!("[VectorTerm] 客户端 {client_id} 请求失败:{e}");
                let _ = mux::write_frame(&mut writer, Msg::Error, e.to_string().as_bytes());
            }
        }
    }

    // 客户端断开:清理它 attach 的所有 pane(取消订阅,不杀进程)。
    detach_all_clients(&server, client_id);
    Ok(())
}

fn dispatch(
    server: &Arc<Server>,
    client_id: u64,
    writer: &mut std::fs::File,
    frame: &Frame,
) -> anyhow::Result<()> {
    match frame.kind {
        Msg::List => {
            let panes = server.panes.lock().unwrap();
            let mut list: Vec<PaneMeta> = panes.values().map(|p| meta_of(&p.lock().unwrap())).collect();
            list.sort_by_key(|m| m.id);
            let mut payload = Vec::new();
            push_u32(&mut payload, list.len() as u32);
            for m in list {
                payload.extend_from_slice(&m.encode());
            }
            mux::write_frame(writer, Msg::ListReply, &payload)?;
        }
        Msg::Spawn => {
            let req = mux::SpawnReq::decode_bytes(&frame.payload)?;
            let id = server.next_pane.fetch_add(1, Ordering::Relaxed);
            let parts = spawn_pty(&req)?;
            let pid = parts.session.pid().unwrap_or(0);
            let pane = Arc::new(Mutex::new(Pane {
                id,
                session: parts.session,
                writer: parts.writer,
                title: String::new(),
                pid,
                buffer: Vec::new(),
                clients: HashMap::new(),
                exited: false,
            }));
            server.panes.lock().unwrap().insert(id, pane.clone());
            // 起输出泵线程:reader → 分发 + 缓存
            spawn_output_pump(pane.clone(), parts.reader);
            // 起退出监视线程
            spawn_exit_watcher(pane.clone(), parts.child, server.clone());

            let meta = meta_of(&pane.lock().unwrap());
            mux::write_frame(writer, Msg::PaneInfo, &meta.encode())?;
        }
        Msg::Attach => {
            let pane_id = u64::from_le_bytes(frame.payload.as_slice().try_into().context("Attach payload")?);
            let pane = {
                let panes = server.panes.lock().unwrap();
                match panes.get(&pane_id).cloned() {
                    Some(p) => p,
                    None => return Err(anyhow::anyhow!("pane {pane_id} 不存在")),
                }
            };
            // 订阅
            pane.lock()
                .unwrap()
                .clients
                .insert(client_id, Arc::new(Mutex::new(Box::new(writer.try_clone().context("clone 写端")?) as Box<dyn Write + Send>)));
            // 回放缓存历史(全量快照)
            let snap = snapshot_of(&pane.lock().unwrap());
            mux::write_frame(writer, Msg::Snapshot, &snap.encode())?;
        }
        Msg::Detach => {
            let pane_id = u64::from_le_bytes(frame.payload.as_slice().try_into().context("Detach payload")?);
            if let Some(pane) = server.panes.lock().unwrap().get(&pane_id) {
                pane.lock().unwrap().clients.remove(&client_id);
            }
            mux::write_frame(writer, Msg::Ok, &[])?;
        }
        Msg::Kill => {
            let pane_id = u64::from_le_bytes(frame.payload.as_slice().try_into().context("Kill payload")?);
            let pane = server.panes.lock().unwrap().remove(&pane_id);
            if let Some(pane) = pane {
                pane.lock().unwrap().session.kill();
            }
            mux::write_frame(writer, Msg::Ok, &[])?;
        }
        Msg::Input => {
            let pane_id = u64::from_le_bytes(frame.payload[..8].try_into().context("Input payload")?);
            let bytes = &frame.payload[8..];
            if let Some(pane) = server.panes.lock().unwrap().get(&pane_id) {
                let mut p = pane.lock().unwrap();
                let _ = p.writer.write_all(bytes);
                let _ = p.writer.flush();
            }
        }
        Msg::Resize => {
            let (pane_id, rows, cols, pw, ph) = decode_resize(&frame.payload)?;
            if let Some(pane) = server.panes.lock().unwrap().get(&pane_id) {
                let _ = pane.lock().unwrap().session.resize(rows, cols, pw, ph);
            }
        }
        _ => {
            // 其余(推送类)不该由客户端发来
        }
    }
    Ok(())
}

/// 输出泵:读 shell 输出 → 广播给所有订阅客户端 + 写进 pane 缓存。
fn spawn_output_pump(pane: Arc<Mutex<Pane>>, mut reader: Box<dyn Read + Send>) {
    let _ = std::thread::Builder::new()
        .name("vectorterm-mux-pump".into())
        .spawn(move || {
            let mut buf = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let data = &buf[..n];
                        // 广播:每个订阅客户端一份 Output 推送
                        let (clients, id) = {
                            let p = pane.lock().unwrap();
                            (p.clients.clone(), p.id)
                        };
                        let payload = mux::OutputPush {
                            pane: id,
                            bytes: data.to_vec(),
                        }
                        .encode();
                        for (_, w) in &clients {
                            let mut w = w.lock().unwrap();
                            let _ = mux::write_frame(&mut **w, Msg::Output, &payload);
                        }
                        // 缓存(供将来 attach 回放)
                        let mut p = pane.lock().unwrap();
                        p.buffer.extend_from_slice(data);
                        if p.buffer.len() > OUTPUT_BUFFER_LIMIT {
                            let excess = p.buffer.len() - OUTPUT_BUFFER_LIMIT;
                            p.buffer.drain(..excess);
                        }
                    }
                }
            }
            // 输出流关闭:标记退出,广播 Exit
            let mut p = pane.lock().unwrap();
            p.exited = true;
            let msg = "输出流已关闭".to_string();
            let mut payload = Vec::with_capacity(8 + msg.len());
            push_u64(&mut payload, p.id);
            payload.extend_from_slice(msg.as_bytes());
            for (_, w) in &p.clients {
                let mut w = w.lock().unwrap();
                let _ = mux::write_frame(&mut **w, Msg::Exit, &payload);
            }
        });
}

/// 子进程退出监视线程。
fn spawn_exit_watcher(
    pane: Arc<Mutex<Pane>>,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    _server: Arc<Server>,
) {
    let _ = std::thread::Builder::new()
        .name("vectorterm-mux-wait".into())
        .spawn(move || {
            let msg = match child.wait() {
                Ok(status) => format!("exit {status}"),
                Err(e) => format!("{e}"),
            };
            let mut p = pane.lock().unwrap();
            p.exited = true;
            let mut payload = Vec::with_capacity(8 + msg.len());
            push_u64(&mut payload, p.id);
            payload.extend_from_slice(msg.as_bytes());
            for (_, w) in &p.clients {
                let mut w = w.lock().unwrap();
                let _ = mux::write_frame(&mut **w, Msg::Exit, &payload);
            }
        });
}

fn meta_of(p: &Pane) -> PaneMeta {
    PaneMeta {
        id: p.id,
        title: p.title.clone(),
        pid: p.pid,
        cwd: None,
        prog: None,
        exited: p.exited,
    }
}

/// 构造一个 pane 的全量快照:把缓存的历史字节作为 raw 回放。
/// 前端拿到后逐字节喂给自己的 Terminal 重建屏幕。
fn snapshot_of(p: &Pane) -> mux::Snapshot {
    mux::Snapshot {
        pane: p.id,
        meta: meta_of(p),
        raw: if p.buffer.is_empty() { None } else { Some(p.buffer.clone()) },
        rows: 0,
        cols: 0,
    }
}

fn detach_all_clients(server: &Arc<Server>, client_id: u64) {
    let panes = server.panes.lock().unwrap();
    for pane in panes.values() {
        pane.lock().unwrap().clients.remove(&client_id);
    }
}

// ---- 编解码辅助(与 mux.rs 重复的最小拷贝,避免跨模块私有访问)----

fn push_u32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn push_u64(v: &mut Vec<u8>, x: u64) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn decode_resize(p: &[u8]) -> anyhow::Result<(u64, u16, u16, u16, u16)> {
    let pane = u64::from_le_bytes(p[..8].try_into().context("resize pane")?);
    let rows = u16::from_le_bytes(p[8..10].try_into()?);
    let cols = u16::from_le_bytes(p[10..12].try_into()?);
    let pw = u16::from_le_bytes(p[12..14].try_into()?);
    let ph = u16::from_le_bytes(p[14..16].try_into()?);
    Ok((pane, rows, cols, pw, ph))
}

// ---- 管道监听 ----

#[cfg(windows)]
/// 创建命名管道的一个监听实例(阻塞等待一个客户端连上)。
/// 每次调用都新建一个实例句柄 —— Named Pipe 的一个实例只服务一个客户端,
/// 服务完必须再 `CreateNamedPipeW` 一个来接下一个。
#[cfg(windows)]
fn create_pipe_instance(first: bool) -> anyhow::Result<windows_sys::Win32::Foundation::HANDLE> {
    use windows_sys::Win32::System::Pipes::{
        CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};

    let name: Vec<u16> = PIPE_NAME.encode_utf16().chain(std::iter::once(0)).collect();
    let open_mode = PIPE_ACCESS_DUPLEX | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            PIPE_UNLIMITED_INSTANCES,
            64 * 1024,
            64 * 1024,
            0,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        anyhow::bail!("创建命名管道失败(错误码 {})", std::io::Error::last_os_error());
    }
    Ok(handle)
}

/// 等待一个客户端连上当前实例,连上后把句柄转成 `File` 返回。
#[cfg(windows)]
fn accept_client(
    handle: windows_sys::Win32::Foundation::HANDLE,
) -> anyhow::Result<std::fs::File> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_PIPE_CONNECTED};
    use windows_sys::Win32::System::Pipes::ConnectNamedPipe;

    unsafe {
        let connected = ConnectNamedPipe(handle, std::ptr::null_mut());
        if connected == 0 {
            let err = GetLastError();
            // 客户端在 ConnectNamedPipe 之前就已完成连接(时序竞争),属正常
            if err != ERROR_PIPE_CONNECTED {
                anyhow::bail!("等待客户端连接失败(错误码 {err})");
            }
        }
        Ok(std::fs::File::from_raw_handle(handle as _))
    }
}

#[cfg(windows)]
fn listen_pipe() -> anyhow::Result<PipeListener> {
    let handle = create_pipe_instance(true)?;
    Ok(PipeListener { handle, first: true })
}

#[cfg(windows)]
struct PipeListener {
    handle: windows_sys::Win32::Foundation::HANDLE,
    first: bool,
}

#[cfg(windows)]
impl PipeListener {
    /// 接受一个客户端,并立即为下一个客户端重建监听实例。
    fn accept(&mut self) -> anyhow::Result<std::fs::File> {
        let file = accept_client(self.handle)?;
        // 服务当前客户端的句柄已移交;为下一个客户端新建实例
        self.handle = create_pipe_instance(false)?;
        self.first = false;
        Ok(file)
    }
}

#[cfg(not(windows))]
fn listen_pipe() -> anyhow::Result<()> {
    anyhow::bail!("mux 守护进程目前仅支持 Windows")
}
