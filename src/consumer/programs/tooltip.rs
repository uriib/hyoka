use iced::{
    Border, Color, Length, Shadow, Theme,
    widget::{container, text},
};
use iced_core::text::Shaping;

use crate::{
    TinyString,
    consumer::{
        AppEvent,
        programs::{Element, Program},
    },
};

pub struct WindowInfo {
    content: String,
}

impl WindowInfo {
    pub fn new(content: String) -> Self {
        Self { content }
    }
}

fn tooltip_text(s: &str, padding: f32, shaping: Shaping) -> Element<'_> {
    let text = text(s).wrapping(text::Wrapping::None).shaping(shaping);
    container(text)
        .style(|theme: &Theme| container::Style {
            text_color: None,
            background: Some(theme.palette().background.into()),
            border: Border::default().rounded(13),
            shadow: Shadow::default(),
            snap: false,
        })
        .padding(padding)
        .center(Length::Shrink)
        .into()
}

impl Program for WindowInfo {
    fn view(&self) -> Element<'_> {
        tooltip_text(self.content.trim_end(), 13.0, Shaping::Auto)
    }
    fn background(&self, _theme: &Theme) -> Color {
        Color::TRANSPARENT
    }
}

pub struct Battery {
    pub content: TinyString,
}

impl Program for Battery {
    fn view(&self) -> Element<'_> {
        tooltip_text(&self.content, 10.0, Shaping::Basic)
    }
    fn background(&self, _theme: &Theme) -> Color {
        Color::TRANSPARENT
        // Color::from_rgba(1.0, 0.0, 0.0, 0.5)
    }
    fn dispatch(&mut self, event: &AppEvent) {
        match event {
            AppEvent::Battery(e) => self.content = e.tooltip(),
            _ => {}
        }
    }
}
