#![allow(dead_code)]

use compio::{driver::ProactorBuilder, runtime::Runtime};
use futures::channel::oneshot;
use iced::{Color, Task, color, theme::Palette};
use iced_runtime::{Action, task};

use crate::wayland::{self, Object};

pub struct Program {}

pub struct State {}

impl State {
    fn update(&mut self, _message: Message) -> iced::Task<Signal> {
        todo!()
    }
    fn view(
        &self,
        _window: iced_core::window::Id,
    ) -> iced::Element<'_, Signal, Theme, iced_tiny_skia::Renderer> {
        todo!()
    }
}

type WlSurface = Object<wayland::ffi::wl_surface>;

#[derive(Debug)]
pub enum Signal {
    Message(Message),
    Command(Command),
}

#[derive(Debug)]
pub enum Message {
    WindowCreated(WlSurface),
}
#[derive(Debug)]
pub enum Command {
    OpenBar { sender: oneshot::Sender<Signal> },
}

// pub fn open_bar() -> Task<WlSurface> {
//     task::oneshot(|channel| Action::Output(Signal::Command(Command::OpenBar { sender: channel })))
// }

pub fn open(settings: iced::window::Settings) -> Task<iced::window::Id> {
    task::oneshot(|channel| {
        Action::Window(iced::window::Action::Open(
            iced::window::Id::unique(),
            settings,
            channel,
        ))
    })
}

pub struct Theme {}

const BACKGROUND: Color = Color::from_rgba8(30, 28, 34, 0.38);

const PURPLE: Color = color!(0xa476f7);
const WHITE: Color = color!(0xcdd6f5);
const GREEN: Color = color!(0x92b673);
const YELLOW: Color = color!(0xe09733);
const RED: Color = color!(0xf25b4f);

impl iced::theme::Base for Theme {
    fn default(preference: iced::theme::Mode) -> Self {
        dbg!(preference);
        Self {}
    }

    fn mode(&self) -> iced::theme::Mode {
        iced::theme::Mode::Dark
    }

    fn base(&self) -> iced::theme::Style {
        iced::theme::Style {
            background_color: BACKGROUND,
            text_color: WHITE,
        }
    }

    fn palette(&self) -> Option<Palette> {
        Some(Palette {
            background: BACKGROUND,
            text: WHITE,
            primary: PURPLE,
            success: GREEN,
            warning: YELLOW,
            danger: RED,
        })
    }

    fn name(&self) -> &str {
        "paper_dark"
    }
}

pub struct Executor {
    runtime: Runtime,
}

impl iced::Executor for Executor {
    fn new() -> Result<Self, futures::io::Error>
    where
        Self: Sized,
    {
        let runtime = Runtime::builder()
            .with_proactor({
                let mut builder = ProactorBuilder::new();
                builder.capacity(16);
                builder.driver_type(compio::driver::DriverType::IoUring);
                builder
            })
            .build()?;
        Ok(Self { runtime })
    }

    fn spawn(
        &self,
        _future: impl Future<Output = ()> + iced_runtime::futures::MaybeSend + 'static,
    ) {
        unimplemented!()
    }

    fn block_on<T>(&self, future: impl Future<Output = T>) -> T {
        self.runtime.block_on(future)
    }
}

impl iced::Program for Program {
    type State = State;

    type Message = Signal;

    type Theme = Theme;

    type Renderer = iced_tiny_skia::Renderer;

    type Executor = Executor;

    fn name() -> &'static str {
        env!("CARGO_PKG_NAME")
    }

    fn settings(&self) -> iced::Settings {
        iced::Settings::default()
    }

    fn window(&self) -> Option<iced_core::window::Settings> {
        None
    }

    fn boot(&self) -> (Self::State, iced::Task<Self::Message>) {
        let init = iced_runtime::task::oneshot(|sender| {
            Action::Output(Signal::Command(Command::OpenBar { sender }))
        });
        (State {}, init)
    }

    fn update(&self, state: &mut Self::State, message: Self::Message) -> iced::Task<Self::Message> {
        match message {
            Signal::Message(message) => state.update(message),
            Signal::Command(_) => unimplemented!(),
        }
    }

    fn view<'a>(
        &self,
        state: &'a Self::State,
        window: iced_core::window::Id,
    ) -> iced_core::Element<'a, Self::Message, Self::Theme, Self::Renderer> {
        state.view(window)
    }

    fn title(&self, _state: &Self::State, _window: iced_core::window::Id) -> String {
        unimplemented!()
    }

    fn subscription(&self, _state: &Self::State) -> iced::Subscription<Self::Message> {
        iced::Subscription::none()
    }

    fn presets(&self) -> &[iced::Preset<Self::State, Self::Message>] {
        &[]
    }
}
