//! mux 协议与客户端 —— wezterm 式「多路复用 / 会话保持」的通信层。
//!
//! 借鉴 wezterm 的 mux 架构,裁剪到 VectorTerm 的单窗口多标签场景:
//!
//! * **守护进程**(`vectorterm --mux-server`)真正持有 ConPTY / shell 子进程和
//!   每个 pane 的 `Terminal`(含滚动缓冲),是「机房」;关掉所有 GUI 后 shell
//!   继续跑,输出持续写进 server 端的滚动缓冲,不丢。
//! * **GUI**(客户端)通过本地命名管道连上守护进程,每个标签是对某个 pane 的
//!   一个「视图」。detach 只是断开/移除视图,pane 在 server 里不受影响;
//!   attach 重新建立视图,历史(滚动缓冲)完整恢复现场。
//!
//! 与 wezterm 的差异(刻意裁剪):
//! * wezterm 用 LEB128 + zstd + TLS/PKI + 共享内存发现,跨机器;VectorTerm 只做
//!   本机单用户,用固定命名管道 `\\.\pipe\vectorterm-mux` + 简单长度前缀帧即可。
//! * wezterm 的 `Terminal` 在前端、server 端只存「稳定行」做脏行 delta;VectorTerm
//!   把整个 `Terminal` 放在 server 端,前端靠「脏行序号 + 逐行快照」同步,省掉
//!   重实现一整套 line 序列化。语义一致:detach 期间输出不丢、attach 历史完整。
//!
//! 协议:小端长度前缀 + 单字节类型 + payload,单向流(请求带 serial,响应回显
//! serial;server 推送用 serial=0,对应 wezterm 的 unilateral notification)。

use anyhow::{bail, Context};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex as StdMutex};

/// 命名管道名。所有 vectorterm 进程(守护进程 + 各 GUI 前端)共享这一个端点。
pub const PIPE_NAME: &str = r"\\.\pipe\vectorterm-mux";

/// 协议版本。客户端/服务端握手时比对,不一致拒绝连接。
pub const PROTOCOL_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// 帧编解码
// ---------------------------------------------------------------------------

/// 消息类型。客户端 → 服务端的请求与服务端 → 客户端的响应/推送共用一套枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Msg {
    /// 握手:payload = 4 字节小端协议版本。双向。
    Hello = 1,
    /// 客户端请求列出所有存活 pane。响应 = ListReply。
    List = 2,
    /// 新建 pane(spawn shell)。payload = SpawnReq。响应 = PaneInfo。
    Spawn = 3,
    /// attach 到已有 pane。payload = 8 字节 pane id。响应 = PaneInfo + 全量快照。
    Attach = 4,
    /// 从 pane 断开视图(不杀进程)。payload = 8 字节 pane id。无响应。
    Detach = 5,
    /// 杀死 pane(shell 及其子进程)。payload = 8 字节 pane id。无响应。
    Kill = 6,
    /// 键盘输入。payload = 8 字节 pane id + 原始字节。无响应。
    Input = 7,
    /// resize。payload = 8 字节 pane id + 4×u16(rows,cols,pw,ph)。无响应。
    Resize = 8,
    /// 服务端推送:pane 有输出(脏行)。payload = OutputPush。
    Output = 9,
    /// 服务端推送:pane 的 shell 退出。payload = 8 字节 pane id + 描述串。
    Exit = 10,
    /// 服务端推送:标题变化。payload = 8 字节 pane id + UTF-8 标题。
    Title = 11,
    /// 响应失败。payload = UTF-8 错误信息。
    Error = 12,
    /// 响应成功(无额外负载)。用于 detach/kill 等。
    Ok = 13,
    /// 响应 = PaneInfo 列表。
    ListReply = 14,
    /// 响应 = PaneInfo(单 pane 元信息)。
    PaneInfo = 15,
    /// 响应 = 全量快照(attach 时一次补齐历史)。payload = Snapshot。
    Snapshot = 16,
}

impl Msg {
    fn from_byte(b: u8) -> Option<Self> {
        use Msg::*;
        Some(match b {
            1 => Hello,
            2 => List,
            3 => Spawn,
            4 => Attach,
            5 => Detach,
            6 => Kill,
            7 => Input,
            8 => Resize,
            9 => Output,
            10 => Exit,
            11 => Title,
            12 => Error,
            13 => Ok,
            14 => ListReply,
            15 => PaneInfo,
            16 => Snapshot,
            _ => return None,
        })
    }
}

/// 一条编好的帧。
#[derive(Debug)]
pub struct Frame {
    pub kind: Msg,
    pub payload: Vec<u8>,
}

/// 读端:从命名管道流里逐帧取。
pub struct FrameReader<R> {
    inner: R,
    /// 半帧缓冲:一个 read 可能只拿回半条消息,剩下的先存这里。
    buf: Vec<u8>,
}

impl<R: Read> FrameReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            buf: Vec::with_capacity(64 * 1024),
        }
    }

    /// 读一条完整帧。流干净关闭时返回 None(EOF)。
    pub fn read_frame(&mut self) -> anyhow::Result<Option<Frame>> {
        loop {
            if self.buf.len() >= 5 {
                let len = u32::from_le_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]])
                    as usize;
                if self.buf.len() >= 5 + len {
                    let kind = Msg::from_byte(self.buf[4])
                        .with_context(|| format!("未知消息类型 {}", self.buf[4]))?;
                    let payload = self.buf[5..5 + len].to_vec();
                    self.buf.drain(..5 + len);
                    return Ok(Some(Frame { kind, payload }));
                }
            }
            // 还需要更多字节
            let mut tmp = [0u8; 16 * 1024];
            let n = self.inner.read(&mut tmp)?;
            if n == 0 {
                // EOF:如果缓冲区里还有半个帧,那是连接被截断
                if self.buf.is_empty() {
                    return Ok(None);
                }
                bail!("连接在帧中间断开(缓冲 {} 字节)", self.buf.len());
            }
            self.buf.extend_from_slice(&tmp[..n]);
        }
    }
}

/// 写一帧到流。长度前缀 + 类型 + payload。
/// 用 `&mut dyn Write` 而非泛型,便于在多线程间传递已装箱的写端。
pub fn write_frame(w: &mut dyn Write, kind: Msg, payload: &[u8]) -> anyhow::Result<()> {
    w.write_all(&(payload.len() as u32).to_le_bytes())?;
    w.write_all(&[kind as u8])?;
    w.write_all(payload)?;
    w.flush()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// payload 编解码辅助
// ---------------------------------------------------------------------------

fn push_u32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn push_u64(v: &mut Vec<u8>, x: u64) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn push_u16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn push_str(v: &mut Vec<u8>, s: &str) {
    push_u32(v, s.len() as u32);
    v.extend_from_slice(s.as_bytes());
}

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}
impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn u32(&mut self) -> anyhow::Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> anyhow::Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes(b.try_into().unwrap()))
    }
    fn u16(&mut self) -> anyhow::Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn str(&mut self) -> anyhow::Result<String> {
        let n = self.u32()? as usize;
        let b = self.take(n)?;
        Ok(String::from_utf8_lossy(b).into_owned())
    }
    fn take(&mut self, n: usize) -> anyhow::Result<&'a [u8]> {
        if self.pos + n > self.buf.len() {
            bail!("payload 越界(需要 {n} 字节,剩 {})", self.buf.len() - self.pos);
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn rest(&self) -> &'a [u8] {
        &self.buf[self.pos..]
    }
}

/// pane 元信息(列表 / attach / spawn 的响应)。
#[derive(Debug, Clone)]
pub struct PaneMeta {
    pub id: u64,
    pub title: String,
    /// shell 进程 id(0 = 已退出或未知)
    pub pid: u32,
    /// 当前工作目录(OSC 7 上报;可选)
    pub cwd: Option<String>,
    /// 前台进程名(可选)
    pub prog: Option<String>,
    /// 是否已退出(退出后仍留在列表里等 kill)
    pub exited: bool,
}

impl PaneMeta {
    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::new();
        push_u64(&mut v, self.id);
        push_str(&mut v, &self.title);
        push_u32(&mut v, self.pid);
        match &self.cwd {
            Some(s) => {
                v.push(1);
                push_str(&mut v, s);
            }
            None => v.push(0),
        }
        match &self.prog {
            Some(s) => {
                v.push(1);
                push_str(&mut v, s);
            }
            None => v.push(0),
        }
        v.push(self.exited as u8);
        v
    }
    fn decode(c: &mut Cursor) -> anyhow::Result<Self> {
        let id = c.u64()?;
        let title = c.str()?;
        let pid = c.u32()?;
        let cwd = match c.take(1)?[0] {
            1 => Some(c.str()?),
            _ => None,
        };
        let prog = match c.take(1)?[0] {
            1 => Some(c.str()?),
            _ => None,
        };
        let exited = c.take(1)?[0] != 0;
        Ok(Self {
            id,
            title,
            pid,
            cwd,
            prog,
            exited,
        })
    }
}

/// Spawn 请求:要起的 shell 与几何尺寸。
#[derive(Debug, Clone)]
pub struct SpawnReq {
    pub shell: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: Vec<(String, String)>,
    pub rows: u16,
    pub cols: u16,
    pub pixel_w: u16,
    pub pixel_h: u16,
    pub dpi: u32,
}

impl SpawnReq {
    /// 从 payload 字节解码(server 端用)。
    pub fn decode_bytes(payload: &[u8]) -> anyhow::Result<Self> {
        let mut c = Cursor::new(payload);
        Self::decode(&mut c)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::new();
        push_str(&mut v, &self.shell);
        push_u32(&mut v, self.args.len() as u32);
        for a in &self.args {
            push_str(&mut v, a);
        }
        match &self.cwd {
            Some(s) => {
                v.push(1);
                push_str(&mut v, s);
            }
            None => v.push(0),
        }
        push_u32(&mut v, self.env.len() as u32);
        for (k, val) in &self.env {
            push_str(&mut v, k);
            push_str(&mut v, val);
        }
        push_u16(&mut v, self.rows);
        push_u16(&mut v, self.cols);
        push_u16(&mut v, self.pixel_w);
        push_u16(&mut v, self.pixel_h);
        push_u32(&mut v, self.dpi);
        v
    }
    fn decode(c: &mut Cursor) -> anyhow::Result<Self> {
        let shell = c.str()?;
        let nargs = c.u32()? as usize;
        let mut args = Vec::with_capacity(nargs);
        for _ in 0..nargs {
            args.push(c.str()?);
        }
        let cwd = match c.take(1)?[0] {
            1 => Some(c.str()?),
            _ => None,
        };
        let nenv = c.u32()? as usize;
        let mut env = Vec::with_capacity(nenv);
        for _ in 0..nenv {
            env.push((c.str()?, c.str()?));
        }
        Ok(Self {
            shell,
            args,
            cwd,
            env,
            rows: c.u16()?,
            cols: c.u16()?,
            pixel_w: c.u16()?,
            pixel_h: c.u16()?,
            dpi: c.u32()?,
        })
    }
}

/// 服务端推送的一批输出。server 端不解析终端序列,直接把 PTY 输出的
/// 原始字节转发给订阅客户端;客户端用自己的 `Terminal` 解析渲染。
#[derive(Debug, Clone)]
pub struct OutputPush {
    pub pane: u64,
    /// 原始输出字节。
    pub bytes: Vec<u8>,
}

impl OutputPush {
    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::new();
        push_u64(&mut v, self.pane);
        push_u32(&mut v, self.bytes.len() as u32);
        v.extend_from_slice(&self.bytes);
        v
    }
    fn decode(c: &mut Cursor) -> anyhow::Result<Self> {
        let pane = c.u64()?;
        let n = c.u32()? as usize;
        let bytes = c.take(n)?.to_vec();
        Ok(Self { pane, bytes })
    }
}

/// attach 时的全量快照:一次把该 pane 的缓冲历史拉过来。
///
/// server 端存的是「原始 PTY 输出字节」(detach 期间缓存),前端拿到后逐字节
/// 喂给自己的 `Terminal` 重建屏幕 —— 终端状态机是确定性的,回放结果与
/// server 端一致。这样跨进程只需要传原始字节,不用序列化 CellAttributes。
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub pane: u64,
    pub meta: PaneMeta,
    /// detach 期间缓存的原始输出字节(若为空则 None)。attach 时回放。
    pub raw: Option<Vec<u8>>,
    /// pane 的可见尺寸(rows/cols),供前端初始建 Terminal。
    pub rows: u16,
    pub cols: u16,
}

impl Snapshot {
    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::new();
        push_u64(&mut v, self.pane);
        v.extend_from_slice(&self.meta.encode());
        match &self.raw {
            Some(b) => {
                v.push(1);
                push_u32(&mut v, b.len() as u32);
                v.extend_from_slice(b);
            }
            None => v.push(0),
        }
        push_u16(&mut v, self.rows);
        push_u16(&mut v, self.cols);
        v
    }
    fn decode(c: &mut Cursor) -> anyhow::Result<Self> {
        let pane = c.u64()?;
        let meta = PaneMeta::decode(c)?;
        let raw = match c.take(1)?[0] {
            1 => {
                let n = c.u32()? as usize;
                Some(c.take(n)?.to_vec())
            }
            _ => None,
        };
        let rows = c.u16()?;
        let cols = c.u16()?;
        Ok(Self {
            pane,
            meta,
            raw,
            rows,
            cols,
        })
    }
}

/// 客户端收到的服务端消息(响应 + 推送的合并视图)。
#[derive(Debug)]
pub enum ServerMsg {
    List(Vec<PaneMeta>),
    PaneInfo(PaneMeta),
    Snapshot(Snapshot),
    Output(OutputPush),
    Exit { pane: u64, msg: String },
    Title { pane: u64, title: String },
    Ok,
    Error(String),
}

/// 解码一条服务端消息。
pub fn decode_server(kind: Msg, payload: &[u8]) -> anyhow::Result<ServerMsg> {
    let mut c = Cursor::new(payload);
    Ok(match kind {
        Msg::ListReply => {
            let n = c.u32()? as usize;
            let mut list = Vec::with_capacity(n);
            for _ in 0..n {
                list.push(PaneMeta::decode(&mut c)?);
            }
            ServerMsg::List(list)
        }
        Msg::PaneInfo => ServerMsg::PaneInfo(PaneMeta::decode(&mut c)?),
        Msg::Snapshot => ServerMsg::Snapshot(Snapshot::decode(&mut c)?),
        Msg::Output => ServerMsg::Output(OutputPush::decode(&mut c)?),
        Msg::Exit => {
            let pane = c.u64()?;
            // 剩余全部字节即描述串(server 端未加长度前缀)
            let msg = String::from_utf8_lossy(c.rest()).into_owned();
            ServerMsg::Exit { pane, msg }
        }
        Msg::Title => {
            let pane = c.u64()?;
            let title = String::from_utf8_lossy(c.rest()).into_owned();
            ServerMsg::Title { pane, title }
        }
        Msg::Ok => ServerMsg::Ok,
        // Error 的 payload 是纯字符串(无长度前缀),直接读剩余字节
        Msg::Error => ServerMsg::Error(String::from_utf8_lossy(c.rest()).into_owned()),
        _ => bail!("意外的服务端消息类型 {kind:?}"),
    })
}

// ---------------------------------------------------------------------------
// 客户端
// ---------------------------------------------------------------------------

/// 到守护进程的一条连接。读写各持一个句柄(命名管道天然双向,
/// 但拆开避免同一线程既读又写阻塞)。
pub struct MuxClient {
    reader: FrameReader<std::fs::File>,
    writer: std::fs::File,
}

impl MuxClient {
    /// 连接守护进程并握手。连接失败(没有守护进程在跑)返回 None。
    pub fn connect() -> anyhow::Result<Option<Self>> {
        let (reader_file, writer) = match try_open_pipe() {
            Some(pair) => pair,
            None => return Ok(None),
        };
        let reader = FrameReader::new(reader_file);
        let mut client = Self { reader, writer };
        client.hello()?;
        Ok(Some(client))
    }

    fn hello(&mut self) -> anyhow::Result<()> {
        write_frame(&mut self.writer, Msg::Hello, &PROTOCOL_VERSION.to_le_bytes())?;
        let Some(frame) = self.reader.read_frame()? else {
            bail!("握手时连接被关闭");
        };
        match frame.kind {
            Msg::Hello => Ok(()),
            Msg::Error => bail!("服务端拒绝:{}", String::from_utf8_lossy(&frame.payload)),
            k => bail!("握手收到意外消息 {k:?}"),
        }
    }

    /// 列出存活 pane。
    pub fn list(&mut self) -> anyhow::Result<Vec<PaneMeta>> {
        write_frame(&mut self.writer, Msg::List, &[])?;
        match self.read_one()? {
            ServerMsg::List(l) => Ok(l),
            ServerMsg::Error(e) => bail!("{e}"),
            m => bail!("list 收到意外响应 {m:?}"),
        }
    }

    /// 新建 pane。返回元信息(全量快照由后续 Output/Snapshot 推送)。
    pub fn spawn(&mut self, req: &SpawnReq) -> anyhow::Result<PaneMeta> {
        write_frame(&mut self.writer, Msg::Spawn, &req.encode())?;
        match self.read_one()? {
            ServerMsg::PaneInfo(m) => Ok(m),
            ServerMsg::Error(e) => bail!("{e}"),
            m => bail!("spawn 收到意外响应 {m:?}"),
        }
    }

    /// attach 到已有 pane,返回全量快照。
    pub fn attach(&mut self, pane: u64) -> anyhow::Result<Snapshot> {
        write_frame(&mut self.writer, Msg::Attach, &pane.to_le_bytes())?;
        match self.read_one()? {
            ServerMsg::Snapshot(s) => Ok(s),
            ServerMsg::Error(e) => bail!("{e}"),
            m => bail!("attach 收到意外响应 {m:?}"),
        }
    }

    /// 键盘输入(同步版,CLI 调试命令用)。
    pub fn input(&mut self, pane: u64, bytes: &[u8]) -> anyhow::Result<()> {
        let mut p = Vec::with_capacity(8 + bytes.len());
        push_u64(&mut p, pane);
        p.extend_from_slice(bytes);
        write_frame(&mut self.writer, Msg::Input, &p)?;
        Ok(())
    }

    fn read_one(&mut self) -> anyhow::Result<ServerMsg> {
        let Some(frame) = self.reader.read_frame()? else {
            bail!("连接被服务端关闭");
        };
        decode_server(frame.kind, &frame.payload)
    }

    /// 拆成「共享写端 + 读端」。前端建好标签后,写端交给输入线程
    /// (键盘 / resize / detach 都是单向 fire-and-forget),读端在独立
    /// 线程持续收服务端推送(Output / Exit / Title)。
    pub fn into_parts(self) -> (Arc<StdMutex<std::fs::File>>, FrameReader<std::fs::File>) {
        (Arc::new(StdMutex::new(self.writer)), self.reader)
    }

    /// 单向发送键盘输入(不等待响应)。
    pub fn send_input(writer: &Arc<StdMutex<std::fs::File>>, pane: u64, bytes: &[u8]) -> anyhow::Result<()> {
        let mut p = Vec::with_capacity(8 + bytes.len());
        push_u64(&mut p, pane);
        p.extend_from_slice(bytes);
        write_frame(&mut *writer.lock().unwrap(), Msg::Input, &p)
    }

    /// 单向发送 resize(不等待响应)。
    pub fn send_resize(
        writer: &Arc<StdMutex<std::fs::File>>,
        pane: u64,
        rows: u16,
        cols: u16,
        pw: u16,
        ph: u16,
    ) -> anyhow::Result<()> {
        let mut p = Vec::new();
        push_u64(&mut p, pane);
        push_u16(&mut p, rows);
        push_u16(&mut p, cols);
        push_u16(&mut p, pw);
        push_u16(&mut p, ph);
        write_frame(&mut *writer.lock().unwrap(), Msg::Resize, &p)
    }

    /// 单向发送 detach(不等待响应)。
    pub fn send_detach(writer: &Arc<StdMutex<std::fs::File>>, pane: u64) -> anyhow::Result<()> {
        let p = pane.to_le_bytes();
        write_frame(&mut *writer.lock().unwrap(), Msg::Detach, &p)
    }

    /// 单向发送 kill(不等待响应)。
    pub fn send_kill(writer: &Arc<StdMutex<std::fs::File>>, pane: u64) -> anyhow::Result<()> {
        let p = pane.to_le_bytes();
        write_frame(&mut *writer.lock().unwrap(), Msg::Kill, &p)
    }
}

/// 把「向某个 pane 写字节」适配成 `std::io::Write`。
///
/// 前端复用现有的 `SharedWriter` + flusher 管线:键盘输入和 Terminal 的
/// 应答字节都流进这里,这里再把它们打包成 Input 帧发给 mux 守护进程。
/// 这样 `spawn_tab` 里本地直连与 mux 两种后端的写路径完全一致。
pub struct MuxWriter {
    pub pane: u64,
    pub writer: Arc<StdMutex<std::fs::File>>,
}

impl std::io::Write for MuxWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        MuxClient::send_input(&self.writer, self.pane, data)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// 打开命名管道(读 + 写两个独立句柄)。守护进程不在时返回 None。
#[cfg(windows)]
fn try_open_pipe() -> Option<(std::fs::File, std::fs::File)> {
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, OPEN_EXISTING};

    // 命名管道双向:开一个 GENERIC_READ|GENERIC_WRITE 句柄,再 try_clone 出读写两端。
    let wide: Vec<u16> = PIPE_NAME.encode_utf16().chain(std::iter::once(0)).collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if handle as isize == -1 {
        return None;
    }
    use std::os::windows::io::FromRawHandle;
    let file = unsafe { std::fs::File::from_raw_handle(handle as _) };
    let reader = file.try_clone().ok()?;
    Some((reader, file))
}

#[cfg(not(windows))]
fn try_open_pipe() -> Option<(std::fs::File, std::fs::File)> {
    None
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, Msg::Hello, &[1, 0, 0, 0]).unwrap();
        write_frame(&mut buf, Msg::Input, b"hello").unwrap();

        let mut r = FrameReader::new(&buf[..]);
        let f = r.read_frame().unwrap().unwrap();
        assert_eq!(f.kind, Msg::Hello);
        assert_eq!(f.payload, vec![1, 0, 0, 0]);

        let f = r.read_frame().unwrap().unwrap();
        assert_eq!(f.kind, Msg::Input);
        assert_eq!(f.payload, b"hello");

        assert!(r.read_frame().unwrap().is_none());
    }

    #[test]
    fn frame_split_across_reads() {
        // 构造一帧,然后按 1 字节喂给 reader,验证半帧缓冲正确
        let mut full = Vec::new();
        write_frame(&mut full, Msg::Title, "标题".as_bytes()).unwrap();

        let mut r = FrameReader::new(&full[..]);
        let f = r.read_frame().unwrap().unwrap();
        assert_eq!(f.kind, Msg::Title);
        assert_eq!(f.payload, "标题".as_bytes());
    }

    #[test]
    fn pane_meta_roundtrip() {
        let m = PaneMeta {
            id: 42,
            title: "hi".into(),
            pid: 1234,
            cwd: Some("C:\\x".into()),
            prog: Some("vim.exe".into()),
            exited: false,
        };
        let binding = m.encode();
        let mut c = Cursor::new(&binding);
        let m2 = PaneMeta::decode(&mut c).unwrap();
        assert_eq!(m2.id, 42);
        assert_eq!(m2.title, "hi");
        assert_eq!(m2.pid, 1234);
        assert_eq!(m2.cwd.as_deref(), Some("C:\\x"));
        assert_eq!(m2.prog.as_deref(), Some("vim.exe"));
        assert!(!m2.exited);
    }

    #[test]
    fn spawn_req_roundtrip_with_unicode() {
        let req = SpawnReq {
            shell: "pwsh.exe".into(),
            args: vec!["-NoLogo".into()],
            cwd: Some("C:\\用户\\测试".into()),
            env: vec![("TERM".into(), "xterm-256color".into())],
            rows: 24,
            cols: 80,
            pixel_w: 800,
            pixel_h: 600,
            dpi: 96,
        };
        let binding = req.encode();
        let mut c = Cursor::new(&binding);
        let r2 = SpawnReq::decode(&mut c).unwrap();
        assert_eq!(r2.shell, "pwsh.exe");
        assert_eq!(r2.args, vec!["-NoLogo"]);
        assert_eq!(r2.cwd.as_deref(), Some("C:\\用户\\测试"));
        assert_eq!(r2.env, vec![("TERM".into(), "xterm-256color".into())]);
        assert_eq!((r2.rows, r2.cols, r2.pixel_w, r2.pixel_h, r2.dpi), (24, 80, 800, 600, 96));
    }

    #[test]
    fn snapshot_roundtrip() {
        let s = Snapshot {
            pane: 7,
            meta: PaneMeta {
                id: 7,
                title: "t".into(),
                pid: 0,
                cwd: None,
                prog: None,
                exited: false,
            },
            raw: Some(b"\x1b[2Jhello".to_vec()),
            rows: 10,
            cols: 20,
        };
        let binding = s.encode();
        let mut c = Cursor::new(&binding);
        let s2 = Snapshot::decode(&mut c).unwrap();
        assert_eq!(s2.pane, 7);
        assert_eq!(s2.raw.as_deref(), Some(&b"\x1b[2Jhello"[..]));
        assert_eq!((s2.rows, s2.cols), (10, 20));
    }

    #[test]
    fn unknown_type_errors() {
        let mut buf = Vec::new();
        write_frame(&mut buf, Msg::Hello, &[]).unwrap();
        buf[4] = 99; // 非法类型
        let mut r = FrameReader::new(&buf[..]);
        assert!(r.read_frame().is_err());
    }
}
