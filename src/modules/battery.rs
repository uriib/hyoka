use std::{fmt::Write as _, io, mem::MaybeUninit, os::fd::OwnedFd};

use compio::{BufResult, io::AsyncRead, net::UnixStream};
use rustix::{
    fs::{Mode, OFlags, SeekFrom},
    net::{
        AddressFamily, SocketType,
        netlink::{KOBJECT_UEVENT, SocketAddrNetlink},
    },
};

use crate::{TinyString, mapping::Mapping};

pub struct Listener {
    name: TinyString,
    stream: UnixStream,
}

impl Listener {
    pub fn new(name: TinyString) -> io::Result<Self> {
        Ok(Self {
            name,
            stream: uevent()?,
        })
    }
    #[allow(dead_code)]
    pub async gen fn stream(&mut self) -> Event {
        let mut buf = Mapping::page().unwrap();

        loop {
            let res;
            BufResult(res, buf) = self.stream.read(buf).await;
            let n = res.unwrap();
            let read = unsafe { buf.as_bytes().get_unchecked(..n) };

            if let Some(e) = parse_listen(read, Some(self.name.as_str())) {
                yield e;
            }
        }
    }
    // TODO: refactor this: check if type changes to battery
    pub async fn listen(&mut self, mut dispatch: impl AsyncFnMut(Event)) {
        let mut buf = Mapping::page().unwrap();

        loop {
            let res;
            BufResult(res, buf) = self.stream.read(buf).await;
            let n = res.unwrap();
            let read = unsafe { buf.as_bytes().get_unchecked(..n) };

            if let Some(e) = parse_listen(read, Some(self.name.as_str())) {
                dispatch(e).await
            }
        }
    }
}

pub fn uevent() -> io::Result<UnixStream> {
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

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Status {
    Charging,
    Full,
    Other,
}

impl Status {
    fn from_bytes(s: &[u8]) -> Self {
        match s {
            b"Charging" => Self::Charging,
            b"Full" => Self::Full,
            _ => Self::Other,
        }
    }
}

macro_rules! builder {
    (@count) => {
        0
    };
    (@count $x0:tt $($xs:tt)* ) => {
        builder!(@count $($xs)*) + 1
    };
    ($(#[$meta:meta])* $pub: vis struct $name:ident($builder:ident) {
        $($vis: vis $field:ident: $type:ty),*$(,)?
    }) => {
        struct $builder {
            $($field: MaybeUninit<$type>),*,
            uninitialized: u8,
        }
        impl $builder {
            fn new() -> Self {
                Self {
                    $($field: MaybeUninit::uninit()),*,
                    uninitialized: builder!(@count $($field)*)
                }
            }
            $(fn $field(&mut self, value: $type) {
                self.$field.write(value);
                self.uninitialized -= 1;
            })*
            fn build(self) -> Option<$name> {
                if self.uninitialized != 0 {
                    return None;
                }
                Some($name {
                    $($field: unsafe { self.$field.assume_init() }),*
                })
            }
        }
        $(#[$meta])*
        $pub struct $name {
            $($vis $field: $type),*
        }
    };
}

builder! {
    #[derive(Debug, Clone, Copy)]
    pub struct Event(EventBuilder) {
        pub status: Status,
        pub power_now: u32,
        pub energy_now: u32,
        pub energy_full: u32,
    }
}

impl Event {
    fn charing(&self) -> bool {
        self.status == Status::Charging
    }
    fn charged(&self) -> bool {
        self.status == Status::Full || self.percentage() > 9900
    }
    fn percentage(&self) -> u32 {
        self.energy_now * 10 / (self.energy_full / 1000)
    }
    fn energy_remaining(&self) -> u32 {
        if self.charing() {
            self.energy_full - self.energy_now
        } else {
            self.energy_now
        }
    }
    fn minutes_remaining(&self) -> u32 {
        if self.power_now == 0 {
            0
        } else {
            self.energy_remaining() * 6 / (self.power_now / 10)
        }
    }
    pub fn icon(&self) -> String {
        if self.charged() {
            "battery-level-100-charged-symbolic".into()
        } else {
            let level = self.percentage() / 1000 * 10;
            let state = if self.charing() { "-charging" } else { "" };
            format!("battery-level-{level}{state}-symbolic")
        }
        // format!("preferences-desktop-keyboard-symbolic")
    }
    pub fn tooltip(&self) -> TinyString {
        let power = self.power_now / 1_000_000;
        let power_frac = (self.power_now - power * 1_000_000) / 10_000;
        let percentage = self.percentage();
        let cap = percentage / 100 + if percentage % 100 > 50 { 1 } else { 0 };
        let h = self.minutes_remaining() / 60;
        let m = self.minutes_remaining() % 60;
        let mut result = TinyString::new();
        write!(&mut result, "{cap}% {power}.{power_frac:0>2}W {h}h{m}").unwrap();
        result
    }
}

fn parse_lines<'a>(lines: impl Iterator<Item = &'a [u8]>, filter: Option<&str>) -> Option<Event> {
    let mut event = EventBuilder::new();
    for line in lines {
        // {
        //     let line = unsafe { str::from_utf8_unchecked(line) };
        //     dbg!(line);
        // }
        let (key, value) = line.split_once(|&x| x == b'=')?;
        match key {
            b"DEVTYPE" => {
                if value != b"power_supply" {
                    return None;
                }
            }
            b"POWER_SUPPLY_NAME" => {
                let value = unsafe { str::from_utf8_unchecked(value) };
                if let Some(filter) = filter {
                    if value != filter {
                        return None;
                    }
                }
            }
            b"POWER_SUPPLY_TYPE" => {
                if value != b"Battery" {
                    return None;
                }
            }
            b"POWER_SUPPLY_STATUS" => event.status(Status::from_bytes(value)),
            b"POWER_SUPPLY_POWER_NOW" => event.power_now(u32::from_ascii(value).ok()?),
            b"POWER_SUPPLY_ENERGY_FULL" => event.energy_full(u32::from_ascii(value).ok()?),
            b"POWER_SUPPLY_ENERGY_NOW" => event.energy_now(u32::from_ascii(value).ok()?),
            _ => {}
        }
        if event.uninitialized == 0 {
            break;
        }
    }
    event.build()
}

fn parse_listen(data: &[u8], filter: Option<&str>) -> Option<Event> {
    let mut lines = data.split(|&x| x == 0);
    let head = lines.next()?;
    // {
    //     let head = unsafe { str::from_utf8_unchecked(head) };
    //     dbg!(head);
    // }
    let (action, _path) = head.split_once(|&x| x == b'@')?;
    if action != b"change" {
        return None;
    }
    parse_lines(lines, filter)
}

pub fn init() -> io::Result<Option<(Event, TinyString, Poll)>> {
    let fd = rustix::fs::open(c"/sys/class/power_supply", OFlags::empty(), Mode::empty())?;
    let mut buf = [MaybeUninit::uninit(); 1024];
    let mut dir = rustix::fs::RawDir::new(&fd, &mut buf);
    while let Some(entry) = dir.next() {
        let entry = entry?;
        // skip . and ..
        if unsafe { *(entry.file_name().as_ptr() as *const u8) } == b'.' {
            continue;
        }
        let name = entry.file_name();
        let device = rustix::fs::openat(&fd, name, OFlags::empty(), Mode::empty())?;
        let uevent = rustix::fs::openat(device, "uevent", OFlags::empty(), Mode::empty())?;
        let mut buf: [MaybeUninit<u8>; _] = [MaybeUninit::uninit(); 1024];
        let (read, _) = rustix::io::read(&uevent, &mut buf)?;
        if let Some(event) = parse_lines(read.split(|&x| x == b'\n'), None) {
            let name = unsafe { str::from_utf8_unchecked(name.to_bytes()) };
            return Ok(Some((event, name.into(), Poll { uevent })));
        }
    }
    Ok(None)
}

pub struct Poll {
    uevent: OwnedFd,
}

impl Poll {
    pub fn poll(&self) -> Option<Event> {
        rustix::fs::seek(&self.uevent, SeekFrom::Start(0)).ok()?;
        let mut buf: [MaybeUninit<u8>; _] = [MaybeUninit::uninit(); 1024];
        let (read, _) = rustix::io::read(&self.uevent, &mut buf).ok()?;
        parse_lines(read.split(|&x| x == b'\n'), None)
    }
}
