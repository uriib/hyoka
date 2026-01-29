use std::{fs, io::Cursor, num::NonZero, path::Path};

use arrayvec::ArrayVec;
use iced::{
    Alignment, Border, Font, Length, Padding, Theme,
    alignment::Vertical,
    widget::{self, button, container, image, mouse_area, row, svg, text, text::Shaping},
};
use lru::LruCache;
use png::{BitDepth, ColorType};
use rustix::{
    fs::{Mode, OFlags},
    mm::{MapFlags, ProtFlags},
    path::Arg as _,
};

use crate::{
    TinyString,
    consumer::{
        Action, AppEvent, StyleSheet,
        programs::{ColorExt as _, Element, Message, Program, Signal},
        theme,
    },
    mapping::Mapping,
    modules::{battery, clock, hyprland},
};

struct BitSet(u16);

impl BitSet {
    fn new() -> Self {
        BitSet(0)
    }
    fn set(&mut self, idx: usize) {
        self.0 |= 1 << idx;
    }
    fn unset(&mut self, idx: usize) {
        self.0 &= !(1 << idx);
    }
    fn get(&self, idx: usize) -> bool {
        (self.0 & (1 << idx)) != 0
    }
}

struct WindowInfo {
    class: TinyString,
    title: TinyString,
    icon: Option<Handle>,
}

pub struct Bar {
    workspaces: BitSet,
    workspace_focused: usize,
    window: WindowInfo,

    battery_icon: Option<Handle>,
    battery_event: Option<battery::Event>,

    date: ArrayVec<u8, 12>,
    time: [u8; 8],
    weekday: &'static str,

    icon_cache: LruCache<TinyString, Option<Handle>, ahash::RandomState>,
}

#[derive(Debug, Clone)]
enum Handle {
    Pixmap(image::Handle),
    Svg(svg::Handle),
}

impl Handle {
    fn load(self) -> Element<'static> {
        self.load_size(24)
    }
    fn load_size(self, size: impl Into<Length> + Copy) -> Element<'static> {
        match self {
            Handle::Pixmap(handle) => image(handle).width(size).into(),
            Handle::Svg(handle) => svg(handle).width(size).height(size).into(),
        }
    }
}

impl Bar {
    fn load_icon(&mut self, key: &TinyString, symbolic: bool) -> Option<Handle> {
        self.icon_cache
            .get_or_insert_ref(key, || {
                cosmic_freedesktop_icons::lookup(key)
                    .with_size(64)
                    // .with_theme("Adwaita")
                    .with_theme("Tela-dracula-dark")
                    // .with_theme("Papirus")
                    .find()
                    .and_then(|path| match path.extension()?.as_encoded_bytes() {
                        b"svg" => {
                            if symbolic {
                                load_symbolic(path, &theme()).map(Handle::Svg)
                            } else {
                                load_svg(path).map(Handle::Svg)
                            }
                        }
                        b"png" => load_png(path).map(Handle::Pixmap),
                        _ => None,
                    })
            })
            .clone()
    }
    pub fn new() -> Self {
        let now = clock::Clock::now();
        Self {
            workspaces: BitSet::new(),
            workspace_focused: usize::MAX,
            window: WindowInfo {
                class: TinyString::new(),
                title: TinyString::new(),
                icon: None,
            },
            battery_icon: None,
            battery_event: None,
            date: now.date(),
            time: now.time(),
            weekday: now.weekday(),
            icon_cache: LruCache::with_hasher(NonZero::new(16).unwrap(), ahash::RandomState::new()),
        }
    }
    fn logo(&self) -> impl Into<Element<'_>> {
        button(
            svg("/usr/share/pixmaps/archlinux-logo.svg")
                .style(|theme: &Theme, status| svg::Style {
                    color: Some(match status {
                        svg::Status::Idle => theme.palette().text,
                        svg::Status::Hovered => theme.palette().primary,
                    }),
                })
                .width(23),
        )
        .style(|_, _| button::Style::default())
        .on_press(Signal::Message(Message::Hello))
        .padding(0)
        .clip(false)
    }
    fn workspace_item(&self, idx: usize) -> Element<'_> {
        let id = (idx + 1) as _;
        let alive = self.workspaces.get(idx);
        let focused = idx == self.workspace_focused;
        let text: Element = if focused {
            text(id % 10).size(11.5).shaping(Shaping::Basic).into()
        } else {
            match alive {
                true => text(id % 10).size(11.5),
                false => text("𒊹")
                    .size(4.5)
                    .font(Font::with_name("Noto Sans Cuneiform")),
            }
            .into()
        };
        let container = container(text).center_x(15).center_y(16);
        let button = button(container)
            .style(move |theme: &Theme, status| button::Style {
                background: match (status, focused) {
                    (button::Status::Hovered, true) => Some(theme.palette().primary.into()),
                    (button::Status::Hovered, false) => {
                        Some(theme.palette().primary.with_alpha(0.18).into())
                    }
                    (_, true) => Some(theme.palette().text.into()),
                    (_, false) => None,
                },
                text_color: match focused {
                    true => theme.palette().background.with_alpha(1.0),
                    false => match status {
                        button::Status::Hovered => theme.palette().primary,
                        _ => theme.palette().text,
                    },
                },
                border: Border::default().rounded(3),
                ..Default::default()
            })
            .padding(0)
            .on_press(Signal::Action(Action::Workspace { id }));
        button.into()
    }
    fn workspace(&self) -> impl Into<Element<'_>> {
        row((0..WORKSPACE_MAX).map(|idx| self.workspace_item(idx)))
            .spacing(2)
            .align_y(Vertical::Center)
    }
    fn title(&self) -> impl Into<Element<'_>> {
        let icon = self.window.icon.clone().map(Handle::load);
        let class = text(self.window.class.as_str())
            .style(|theme: &Theme| text::Style {
                color: Some(theme.palette().primary),
            })
            .size(14.5)
            .shaping(Shaping::Basic);
        let title = text(self.window.title.as_str());
        let row = row([icon.into(), class.into(), title.into()])
            .align_y(Vertical::Center)
            .spacing(5);

        mouse_area(row)
            // .on_enter(Signal::Message(Message::Hello))
            // .on_exit(Signal::Message(Message::Bye))
            .on_enter(Signal::Action(Action::WindowInfo))
            .on_exit(Signal::Action(Action::CloseTooltip))
    }
    fn battery(&self) -> Option<Element<'_>> {
        let icon = self.battery_icon.clone().map(|x| x.load_size(17))?;
        Some(if let Some(ref e) = self.battery_event {
            mouse_area(icon)
                .on_enter(Signal::Action(Action::Battery(e.tooltip())))
                .on_exit(Signal::Action(Action::CloseTooltip))
                .into()
        } else {
            icon.into()
        })
    }
    fn clock(&self) -> impl Into<Element<'_>> {
        let date = text(unsafe { str::from_utf8_unchecked(&self.date) })
            .size(12.5)
            .height(Length::Fill)
            .align_y(Alignment::End);
        let date = container(date)
            .padding(Padding::default().bottom(7.5))
            .into();
        let time = text(unsafe { str::from_utf8_unchecked(&self.time) })
            .size(17)
            .height(Length::Fill)
            .shaping(Shaping::Basic)
            .center()
            .width(64)
            .align_x(Alignment::Center)
            .into();
        let weekday = text(self.weekday).size(15).height(Length::Fill).center();
        let weekday = container(weekday)
            .padding(Padding::default().bottom(4.5))
            .into();
        row([date, time, weekday]).spacing(7)
    }
}

const WORKSPACE_MAX: usize = 10;

impl Program for Bar {
    fn view(&self) -> Element<'_> {
        let left = widget::row![
            self.logo().into(),
            self.workspace().into(),
            self.title().into()
        ]
        .align_y(Vertical::Center)
        .spacing(7)
        .padding(Padding::new(0.0).left(16))
        .width(Length::Fill)
        .height(Length::Fill);

        let right = widget::row![self.battery(), self.clock().into()]
            .align_y(Vertical::Center)
            .padding(Padding::new(0.0).right(13))
            .spacing(9)
            .height(Length::Fill);

        widget::row![left, right].into()
    }
    fn dispatch(&mut self, event: &AppEvent) {
        match event {
            AppEvent::Hyprland(event) => match event {
                hyprland::Event::Workspace { id } => self.workspace_focused = id - 1,
                hyprland::Event::CreateWorkspace { id } => self.workspaces.set(id - 1),
                hyprland::Event::DestroyWorkspace { id } => self.workspaces.unset(id - 1),
                hyprland::Event::ActiveWindow { class, title } => {
                    self.window = WindowInfo {
                        icon: if class.is_empty() {
                            None
                        } else {
                            self.load_icon(&class, false)
                        },
                        class: truncate(class.clone(), 15, "…"),
                        title: truncate(title.clone(), 50, "…"),
                    }
                }
            },
            AppEvent::Battery(e) => {
                self.battery_event = Some(e.clone());
                self.battery_icon = self.load_icon(&e.icon().into(), true);
            }
            AppEvent::Clock(e) => {
                self.date = e.date();
                self.time = e.time();
                self.weekday = e.weekday();
            }
        }
    }
}

fn load_svg(path: impl AsRef<Path>) -> Option<svg::Handle> {
    let path = path.as_ref();
    let fd = rustix::fs::open(path, OFlags::CLOEXEC, Mode::empty()).unwrap();
    let mapping = Mapping::map(fd, ProtFlags::READ, MapFlags::PRIVATE).unwrap();
    let text = unsafe { str::from_utf8_unchecked(mapping.as_bytes()) };
    let tree = usvg::Tree::from_str(text, &usvg::Options::default())
        .inspect_err(|err| log::warn!("cannot parse {path:?}: {err:?}"))
        .ok()?;
    Some(svg::Handle::from_tree(tree))
}

fn load_symbolic(path: impl AsRef<Path>, theme: &Theme) -> Option<svg::Handle> {
    let path = path.as_ref();
    let data = fs::read_to_string(path)
        .unwrap()
        .replace("currentColor", &theme.palette().text.to_string());
    let tree = usvg::Tree::from_str(
        &data,
        &usvg::Options {
            style_sheet: Some(theme.css_injection()),
            ..Default::default()
        },
    )
    .inspect_err(|err| log::warn!("cannot parse {path:?}: {err:?}"))
    .ok()?;
    Some(svg::Handle::from_tree(tree))
}

fn load_png(path: impl AsRef<Path>) -> Option<image::Handle> {
    let path = path.as_ref();
    let path = path.as_cow_c_str().unwrap();
    let fd = rustix::fs::open(path.as_c_str(), OFlags::CLOEXEC, Mode::empty()).ok()?;
    let data = Mapping::map(fd, ProtFlags::READ, MapFlags::PRIVATE).ok()?;
    let cursor = Cursor::new(data.as_bytes());
    let decoder = png::Decoder::new(cursor);

    let mut reader = decoder.read_info().unwrap();
    let len = reader.output_buffer_size().unwrap();
    let buf = Mapping::anon(len, ProtFlags::READ | ProtFlags::WRITE, MapFlags::PRIVATE).unwrap();
    let info = reader
        .next_frame(buf.as_bytes_mut())
        .inspect_err(|e| log::warn!("cannot decode {path:?}: {e}"))
        .ok()?;
    match info.color_type {
        ColorType::Rgba => {}
        x => {
            log::warn!("{path:?} has unsupported color type: {x:?}");
            return None;
        }
    }
    match info.bit_depth {
        BitDepth::Eight => {}
        x => {
            log::warn!("{path:?} has unsupported {}-bit depth", x as u32);
            return None;
        }
    }
    let handle = image::Handle::from_rgba(info.width, info.height, buf);
    Some(handle)
}

fn truncate(mut s: TinyString, mut len: usize, ellipsis: &str) -> TinyString {
    if s.len() < len {
        s
    } else {
        len -= ellipsis.len();
        while !s.is_char_boundary(len) {
            len -= 1
        }
        s.truncate(len);
        s.push_str(ellipsis);
        s
    }
}
