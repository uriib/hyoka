use std::{io, mem::MaybeUninit, os::fd::OwnedFd};

use compio::{io::AsyncRead, net::UnixStream};
use rustix::{
    fs::{Mode, OFlags},
    net::{
        AddressFamily, SocketType,
        netlink::{KOBJECT_UEVENT, SocketAddrNetlink},
    },
};

use crate::{mapping::Mapping, modules::battery};

#[derive(Debug)]
pub enum Event {
    PowerOnline,
    PowerOffline,
    BatCapacity(u8),
    BatStatus(battery::Status),
}

fn uevent() -> io::Result<UnixStream> {
    let fd = rustix::net::socket(
        AddressFamily::NETLINK,
        SocketType::RAW,
        Some(KOBJECT_UEVENT),
    )?;
    rustix::net::bind(&fd, &SocketAddrNetlink::new(0, 1))?;
    let stream = std::os::unix::net::UnixStream::from(fd);
    let stream = compio::net::UnixStream::from_std(stream).unwrap();
    Ok(stream)
}

pub fn new() -> Listener {
    Listener {
        stream: uevent().unwrap(),
    }
}

pub struct Listener {
    stream: UnixStream,
}

impl Listener {
    pub async fn serve(mut self, dispatch: impl AsyncFnMut(Event) + Clone) {
        #[derive(Debug)]
        enum Subsystem {
            Backlight,
            PowerSupply,
        }
        let mut buf = Mapping::page().unwrap();
        let cb = dispatch;
        let mut dispatch = cb.clone();
        loop {
            let n;
            (n, buf) = self.stream.read(buf).await.unwrap();
            let msg = unsafe { buf.as_bytes().get_unchecked(..n) };
            let (_, body) = parse_message(msg).unwrap();
            let mut subsystem = None;
            let mut is_battery = false;
            let mut ac_online = None;
            let mut capacity = None;
            let mut status = None;
            for (k, v) in body {
                match k {
                    "SUBSYSTEM" => match v {
                        "backlight" => subsystem = Some(Subsystem::Backlight),
                        "power_supply" => subsystem = Some(Subsystem::PowerSupply),
                        _ => {}
                    },
                    "POWER_SUPPLY_TYPE" => match v {
                        "Battery" => is_battery = true,
                        _ => {}
                    },
                    "POWER_SUPPLY_ONLINE" => match v {
                        "0" => ac_online = Some(false),
                        _ => ac_online = Some(true),
                    },
                    "POWER_SUPPLY_CAPACITY" => {
                        capacity = Some(u8::from_ascii(v.as_bytes()).unwrap());
                    }
                    "POWER_SUPPLY_STATUS" => {
                        status = Some(battery::Status::from_bytes(v.as_bytes()));
                    }
                    _ => {}
                }
            }
            match subsystem {
                Some(Subsystem::PowerSupply) => {
                    if is_battery {
                        if let Some(x) = capacity {
                            dispatch(Event::BatCapacity(x)).await
                        }
                        if let Some(x) = status {
                            dispatch(Event::BatStatus(x)).await
                        }
                    } else {
                        if let Some(x) = ac_online {
                            dispatch(if x {
                                Event::PowerOnline
                            } else {
                                Event::PowerOffline
                            })
                            .await;
                        }
                    }
                }

                Some(Subsystem::Backlight) => {}
                _ => {}
            }
        }
    }
}

fn parse_message(data: &[u8]) -> Option<(&str, impl Iterator<Item = (&str, &str)>)> {
    let mut lines = data.split(|&x| x == 0);
    let head = unsafe { str::from_utf8_unchecked(lines.next()?) };
    let lines = lines
        .map_while(|x| x.split_once(|&x| x == b'='))
        .map(|(k, v)| unsafe { (str::from_utf8_unchecked(k), str::from_utf8_unchecked(v)) });
    Some((head, lines))
}

#[allow(dead_code)]
fn backlight() -> Option<OwnedFd> {
    let fd = rustix::fs::open(c"/sys/class/backlight", OFlags::empty(), Mode::empty()).ok()?;
    let mut buf = [MaybeUninit::uninit(); 1024];
    let mut dir = rustix::fs::RawDir::new(&fd, &mut buf);
    while let Some(entry) = dir.next() {
        let entry = entry.unwrap();

        // skip . and ..
        if unsafe { *(entry.file_name().as_ptr() as *const u8) } == b'.' {
            continue;
        }
        let name = entry.file_name();
        let device = rustix::fs::openat(&fd, name, OFlags::empty(), Mode::empty()).unwrap();
        return Some(device);
    }
    None
}
