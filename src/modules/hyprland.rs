use std::{
    env,
    io::{self, Cursor, Write as _},
    mem::MaybeUninit,
    os::{
        fd::{AsFd, AsRawFd as _, BorrowedFd, FromRawFd as _, IntoRawFd, OwnedFd},
        unix::net,
    },
    path::PathBuf,
};

use rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType};
// use tokio::io::AsyncReadExt as _;

use crate::mapping::Mapping;

pub struct Context {
    /// hyprland instance signature
    his: PathBuf,
    listener: OwnedFd,
}

fn xdg_runtime_dir() -> PathBuf {
    env::var_os("XDG_RUNTIME_DIR")
        .map(Into::into)
        .unwrap_or_else(|| format!("/run/user/{}", rustix::process::getuid()).into())
}

impl Context {
    pub fn new() -> Option<Self> {
        let mut dir = xdg_runtime_dir();
        dir.push("hypr");
        dir.push(env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?);
        let his = dir;
        let socket = rustix::net::socket_with(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
            None,
        )
        .unwrap();
        rustix::net::connect(
            &socket,
            &SocketAddrUnix::new(his.join(".socket2.sock")).unwrap(),
        )
        .unwrap();
        Some(Self {
            his,
            listener: socket,
        })
    }
    fn controller(&self) -> Controller {
        let socket = rustix::net::socket_with(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
            None,
        )
        .unwrap();
        rustix::net::connect(
            &socket,
            &SocketAddrUnix::new(self.his.join(".socket.sock")).unwrap(),
        )
        .unwrap();
        Controller { socket }
    }
    pub async fn request<'a>(
        &self,
        req: &[u8],
        res: &'a mut [MaybeUninit<u8>],
    ) -> io::Result<&'a str> {
        self.controller().request(req, res).await
    }
    pub fn command_quiet(&self, cmd: Command) -> io::Result<()> {
        let mut buf = MaybeUninit::<[u8; 64]>::uninit();
        let buf = unsafe { buf.assume_init_mut() };
        let mut buf = Cursor::new(&mut buf[..]);
        match cmd {
            Command::Workspace(id) => {
                write!(&mut buf, "q/dispatch workspace {id}").unwrap();
                let writen = buf.position() as usize;
                self.controller()
                    .request_quiet(&buf.into_inner()[..writen])?;
            }
        }
        Ok(())
    }
    pub fn listener(&self) -> Listener<'_> {
        Listener {
            socket: self.listener.as_fd(),
        }
    }
}

pub trait Handler {
    async fn workspace(&self, id: usize);
    async fn create_workspace(&self, id: usize);
    async fn destroy_workspace(&self, id: usize);
    async fn active_window(&self, class: &str, title: &str);
}

pub struct Listener<'a> {
    socket: BorrowedFd<'a>,
}

pub struct Controller {
    socket: OwnedFd,
}

impl Listener<'_> {
    pub async fn listen(self, mut handler: impl Handler) {
        let mut stream = tokio::net::UnixStream::from_std(unsafe {
            net::UnixStream::from_raw_fd(self.socket.as_raw_fd())
        })
        .unwrap();
        let buf = Mapping::page().unwrap();

        fn parse_workspace(body: &[u8]) -> Option<usize> {
            let (id, _name) = body.split_once(|&x| x == b',')?;
            usize::from_ascii(id).ok()
        }

        async fn parse_line(line: &[u8], handler: &mut impl Handler) -> Option<()> {
            let idx = line.iter().position(|&x| x == b'>')?;
            let event_type = unsafe { line.get_unchecked(..idx) };
            let event_body = unsafe { line.get_unchecked(idx + 2..) };
            match event_type {
                b"workspacev2" => {
                    handler.workspace(parse_workspace(event_body)?).await;
                    Some(())
                }
                b"createworkspacev2" => {
                    handler.create_workspace(parse_workspace(event_body)?).await;
                    Some(())
                }
                b"destroyworkspacev2" => {
                    handler
                        .destroy_workspace(parse_workspace(event_body)?)
                        .await;
                    Some(())
                }
                b"activewindow" => {
                    let (class, title) = event_body.split_once(|&x| x == b',')?;
                    let [class, title] =
                        [class, title].map(|x| unsafe { str::from_utf8_unchecked(x) });
                    handler.active_window(class, title).await;
                    Some(())
                }
                _ => None,
            }
        }

        loop {
            stream.readable().await.unwrap();
            let n = rustix::io::read(stream.as_fd(), buf.as_bytes_mut()).unwrap();
            let buf = unsafe { buf.as_bytes_mut().get_unchecked(..n) };

            for line in buf.split(|&x| x == b'\n') {
                parse_line(line, &mut handler).await;
            }
        }
    }
}

impl Controller {
    async fn request<'a>(self, req: &[u8], res: &'a mut [MaybeUninit<u8>]) -> io::Result<&'a str> {
        rustix::io::write(&self.socket, req)?;
        let mut stream = tokio::net::UnixStream::from_std(unsafe {
            net::UnixStream::from_raw_fd(self.socket.into_raw_fd())
        })?;
        let n = stream.read(unsafe { res.assume_init_mut() }).await?;
        Ok(unsafe { str::from_utf8_unchecked(res.get_unchecked(0..n).assume_init_ref()) })
    }
    fn request_quiet(self, req: &[u8]) -> io::Result<()> {
        rustix::io::write(&self.socket, req)?;
        Ok(())
    }
}

pub fn parse_workspace_id(data: &str) -> Option<usize> {
    let data = data.as_bytes();
    let span = data.get(13..)?;
    let pos = span.iter().position(|&x| x == b' ')?;
    usize::from_ascii(&span[..pos]).ok()
}

pub enum Command {
    Workspace(u8),
}
