use std::{cell::RefCell, io::Cursor, num::NonZero, path::Path};

use iced::{
    Border, Font, Length, Padding, Shadow, Theme,
    alignment::Vertical,
    widget::{self, button, container, image, mouse_area, row, svg, text},
};
use lru::LruCache;
use png::ColorType;
use rustix::{
    fs::{Mode, OFlags},
    mm::{MapFlags, ProtFlags},
    path::Arg as _,
};

use crate::{
    TinyString,
    consumer::{
        Action, AppEvent,
        programs::{ColorExt as _, Element, Message, Program, Signal},
    },
    mapping::Mapping,
    modules::hyprland,
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

    icon_cache: RefCell<LruCache<TinyString, Option<Handle>, ahash::RandomState>>,
}

#[derive(Clone)]
enum Handle {
    Pixmap(iced_core::image::Handle),
    Svg(iced_core::svg::Handle),
}

impl Bar {
    fn load_icon(&self, key: &TinyString) -> Option<Handle> {
        if key.is_empty() {
            return None;
        }
        self.icon_cache
            .borrow_mut()
            .get_or_insert_ref(key, || {
                cosmic_freedesktop_icons::lookup(key)
                    .with_size(1024)
                    .with_theme("Tela-dracula-dark")
                    .find()
                    .and_then(|path| match path.extension()?.as_encoded_bytes() {
                        b"svg" => Some(Handle::Svg(iced_core::svg::Handle::from_path(path))),
                        b"png" => png(path).map(Handle::Pixmap),
                        _ => None,
                    })
            })
            .clone()
    }
    pub fn new() -> Self {
        Self {
            workspaces: BitSet::new(),
            workspace_focused: usize::MAX,
            window: WindowInfo {
                class: TinyString::new(),
                title: TinyString::new(),
                icon: None,
            },
            icon_cache: RefCell::new(LruCache::with_hasher(
                NonZero::new(16).unwrap(),
                ahash::RandomState::new(),
            )),
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
            text(id % 10).size(11.5).into()
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
        let mut row = row([]).align_y(Vertical::Center);
        if let Some(icon) = self.window.icon.clone() {
            let icon: Element = match icon {
                Handle::Pixmap(handle) => image(handle).width(24).into(),
                Handle::Svg(handle) => svg(handle).width(24).into(),
            };
            row = row.push(icon);
        }

        let inner = row
            .push(
                text(self.window.class.as_str())
                    .style(|theme: &Theme| text::Style {
                        color: Some(theme.palette().primary),
                    })
                    .size(14.5),
            )
            .push(text(self.window.title.as_str()))
            .spacing(5);

        mouse_area(inner)
            // .on_enter(Signal::Message(Message::Hello))
            // .on_exit(Signal::Message(Message::Bye))
            .on_enter(Signal::Action(Action::WindowInfo))
            .on_exit(Signal::Action(Action::CloseWindowInfo))
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
        .height(Length::Fill);

        let row = widget::row![left].width(Length::Fill);
        row.into()
        // container(row)
        //     .style(|theme: &Theme| container::Style {
        //         text_color: None,
        //         background: Some(theme.palette().background.into()),
        //         border: Border::default(),
        //         shadow: Shadow::default(),
        //         snap: false,
        //     })
        //     .into()
    }
    fn update(&mut self, message: Message) {
        dbg!(message);
    }
    fn dispatch(&mut self, event: AppEvent) {
        match event {
            AppEvent::Hyprland(event) => match event {
                hyprland::Event::Workspace { id } => self.workspace_focused = id - 1,
                hyprland::Event::CreateWorkspace { id } => self.workspaces.set(id - 1),
                hyprland::Event::DestroyWorkspace { id } => self.workspaces.unset(id - 1),
                hyprland::Event::ActiveWindow { class, title } => {
                    self.window = WindowInfo {
                        icon: self.load_icon(&class),
                        class: truncate(class, 15, "…"),
                        title: truncate(title, 50, "…"),
                    }
                }
            },
        }
    }
}

fn png(path: impl AsRef<Path>) -> Option<iced_core::image::Handle> {
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
        png::BitDepth::Eight => {}
        x => {
            log::warn!("{path:?} has unsupported {}-bit depth", x as u32);
            return None;
        }
    }
    let handle = iced_core::image::Handle::from_rgba(info.width, info.height, buf);
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
