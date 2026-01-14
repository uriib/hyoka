use iced::{Color, Theme};
use iced_tiny_skia::Renderer;

use crate::consumer::{Action, AppEvent};

pub trait Program {
    fn view(&self) -> Element<'_>;
    fn update(&mut self, message: Message) {
        _ = message;
    }
    fn dispatch(&mut self, event: &AppEvent) {
        _ = event;
    }
    fn background(&self, theme: &Theme) -> Color {
        theme.palette().background
    }
}

#[derive(Debug, Clone)]
pub enum Signal {
    Message(Message),
    Action(Action),
}

#[derive(Debug, Clone)]
pub enum Message {
    Hello,
    Bye,
}

pub type UserInterface<'ui> = iced_runtime::UserInterface<'ui, Signal, Theme, Renderer>;
pub type Element<'ui> = iced::Element<'ui, Signal, Theme, Renderer>;

pub use bar::Bar;

pub trait ColorExt {
    fn with_alpha(self, a: f32) -> Self;
}

impl ColorExt for Color {
    fn with_alpha(self, a: f32) -> Self {
        let Self { r, g, b, a: _ } = self;
        Self { r, g, b, a }
    }
}

mod bar;
pub mod tooltip;
