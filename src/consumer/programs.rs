use iced::{
    Border, Color, Length, Shadow, Theme,
    widget::{container, text},
};
use iced_tiny_skia::Renderer;

use crate::consumer::{Action, AppEvent};

pub trait Program {
    fn view(&self) -> Element<'_>;
    fn update(&mut self, message: Message);
    fn dispatch(&mut self, event: AppEvent) {
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
pub struct WindowInfo {
    content: String,
}

impl WindowInfo {
    pub fn new(content: String) -> Self {
        let content = content.replace("\t", "        ");
        Self { content }
    }
}

impl Program for WindowInfo {
    fn view(&self) -> Element<'_> {
        let text = text(self.content.trim_end()).wrapping(text::Wrapping::None);
        container(text)
            .style(|theme: &Theme| container::Style {
                text_color: None,
                background: Some(theme.palette().background.into()),
                border: Border::default().rounded(13),
                shadow: Shadow::default(),
                snap: false,
            })
            .padding(13.0)
            .center(Length::Shrink)
            .into()
    }

    fn update(&mut self, _message: Message) {}
    fn background(&self, _theme: &Theme) -> Color {
        Color::TRANSPARENT
    }
}

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
