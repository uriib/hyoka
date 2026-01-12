use std::{env, io::Write, path::PathBuf};

use compio::{
    BufResult,
    buf::IoBuf,
    io::{AsyncRead as _, AsyncReadExt, AsyncWrite as _},
    net::UnixStream,
};

use crate::{
    TinyString,
    mapping::Mapping,
    sync::mpsc::{self, Receiver, Sender},
};

fn xdg_runtime_dir() -> PathBuf {
    env::var_os("XDG_RUNTIME_DIR")
        .map(Into::into)
        .unwrap_or_else(|| format!("/run/user/{}", rustix::process::getuid()).into())
}

pub struct Context {
    /// hyprland instance signature
    his: PathBuf,
}

impl Context {
    pub fn new() -> Option<Self> {
        let mut dir = xdg_runtime_dir();
        dir.push("hypr");
        dir.push(env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?);
        let his = dir;
        Some(Self { his })
    }
    pub async fn controller(&self) -> Controller {
        Controller {
            stream: UnixStream::connect(self.his.join(".socket.sock"))
                .await
                .unwrap(),
        }
    }
    async fn listener(&self) -> Listener {
        Listener {
            stream: UnixStream::connect(self.his.join(".socket2.sock"))
                .await
                .unwrap(),
        }
    }
}

pub struct Listener {
    stream: UnixStream,
}

pub struct Controller {
    stream: UnixStream,
}

#[derive(Debug)]
pub enum Event {
    Workspace {
        id: usize,
    },
    CreateWorkspace {
        id: usize,
    },
    DestroyWorkspace {
        id: usize,
    },
    ActiveWindow {
        class: TinyString,
        title: TinyString,
    },
}

impl Listener {
    pub async fn listen(mut self, mut dispatch: impl AsyncFnMut(Event)) {
        let mut buffer = Mapping::page().unwrap();

        fn parse_workspace(body: &[u8]) -> Option<usize> {
            let (id, _name) = body.split_once(|&x| x == b',')?;
            usize::from_ascii(id).ok()
        }

        async fn parse_line(line: &[u8], dispatch: &mut impl AsyncFnMut(Event)) -> Option<()> {
            let idx = line.iter().position(|&x| x == b'>')?;
            let event_type = unsafe { line.get_unchecked(..idx) };
            let event_body = unsafe { line.get_unchecked(idx + 2..) };
            match event_type {
                b"workspacev2" => {
                    dispatch(Event::Workspace {
                        id: parse_workspace(event_body)?,
                    })
                    .await;
                    Some(())
                }
                b"createworkspacev2" => {
                    dispatch(Event::CreateWorkspace {
                        id: parse_workspace(event_body)?,
                    })
                    .await;
                    Some(())
                }
                b"destroyworkspacev2" => {
                    dispatch(Event::DestroyWorkspace {
                        id: parse_workspace(event_body)?,
                    })
                    .await;
                    Some(())
                }
                b"activewindow" => {
                    let (class, title) = event_body.split_once(|&x| x == b',')?;
                    let [class, title] =
                        [class, title].map(|x| unsafe { str::from_utf8_unchecked(x) }.into());
                    dispatch(Event::ActiveWindow { class, title }).await;
                    Some(())
                }
                _ => None,
            }
        }

        loop {
            let BufResult(result, buf) = self.stream.read(buffer).await;
            buffer = buf;
            let n = result.unwrap();
            let buf = unsafe { buffer.as_bytes_mut().get_unchecked(..n) };

            for line in buf.split(|&x| x == b'\n') {
                parse_line(line, &mut dispatch).await;
            }
        }
    }
}

#[derive(Clone)]
pub enum Command {
    Workspace(u8),
}

#[derive(Clone)]
pub enum Request {
    ActiveWindow,
}

// #[derive(Clone)]
// pub enum Method {
//     Command(Command),
//     Request(Request),
// }

#[derive(Debug)]
#[must_use]
pub enum Response {
    Raw(String),
}

impl Controller {
    /// similar to request, but requires no response.
    pub async fn command(mut self, cmd: Command) {
        let mut buf = Vec::with_capacity(64);
        match cmd {
            Command::Workspace(id) => {
                write!(&mut buf, "q/dispatch workspace {id}").unwrap();
            }
        }
        self.stream.write(buf).await.unwrap();
    }

    pub async fn raw_request(mut self, msg: impl IoBuf) -> String {
        self.stream.write(msg).await.unwrap();

        let buf = Vec::with_capacity(1024);
        let BufResult(_, buf) = self.stream.read_to_end(buf).await;
        unsafe { String::from_utf8_unchecked(buf) }
    }

    pub async fn request(self, req: Request) -> Response {
        let msg = match req {
            Request::ActiveWindow => b"activewindow",
        };
        let raw = self.raw_request(msg).await;
        Response::Raw(raw)
    }
}

// #[derive(Debug)]
// pub enum Event {
//     Message(Message),
//     Response(Response),
// }

pub struct Client {
    pub context: Context,
    // pub request: Sender<Method>,
    pub events: Receiver<Event>,
}

pub struct Server {
    // context: Context,
    listener: Listener,
    // requests: Receiver<Method>,
    events: Sender<Event>,
}

impl Server {
    pub async fn run(self, init: Controller) {
        let Self {
            listener,
            mut events,
        } = self;

        let res = init
            .raw_request("[[BATCH]]workspaces;activeworkspace;activewindow")
            .await;
        let mut res = res.split("\n\n\n\n\n");
        if let Some(workspaces) = res.next() {
            for workspace in workspaces.split("\n\n") {
                if let Some(id) = parse_workspace_id(workspace) {
                    events.send(Event::CreateWorkspace { id }).await.unwrap();
                }
            }
        }
        if let Some(active_workspace) = res.next() {
            if let Some(id) = parse_workspace_id(active_workspace) {
                events.send(Event::Workspace { id }).await.unwrap();
            }
        }
        if let Some(active_window) = res.next() {
            let mut required = 2;
            let [mut class, mut title] = [TinyString::new(), TinyString::new()];
            for line in active_window.split("\n") {
                if line.starts_with("\tclass") {
                    if let Some(pos) = line.find(' ') {
                        class = line[pos + 1..].into();
                        required -= 1;
                    }
                } else if line.starts_with("\ttitle") {
                    if let Some(pos) = line.find(' ') {
                        title = line[pos + 1..].into();
                        required -= 1;
                    }
                }
                if required == 0 {
                    break;
                }
            }
            events
                .send(Event::ActiveWindow { class, title })
                .await
                .unwrap();
        }

        listener
            .listen(async |msg| {
                events.send(msg).await.unwrap();
            })
            .await;
    }
}

pub async fn new() -> Option<(Server, Client)> {
    let context = Context::new()?;
    let listener = context.listener().await;
    let (sender, receiver) = mpsc::channel(1);

    Some((
        Server {
            listener,
            events: sender,
        },
        Client {
            context,
            events: receiver,
        },
    ))
}

pub fn parse_workspace_id(data: &str) -> Option<usize> {
    let data = data.as_bytes();
    let span = data.get(13..)?;
    let pos = span.iter().position(|&x| x == b' ')?;
    usize::from_ascii(&span[..pos]).ok()
}
