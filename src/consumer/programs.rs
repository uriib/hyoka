use std::rc::Rc;

use iced::{
    Theme,
    widget::{button, svg},
};
use iced_tiny_skia::Renderer;

use crate::consumer::Runner;

pub trait Program {
    fn view(&self) -> Element<'_>;
    fn update(&mut self, message: Message);
}

#[derive(Clone)]
pub enum Signal {
    Message(Message),
    Action(Rc<dyn Fn(&mut Runner)>),
}

#[derive(Debug, Clone)]
pub enum Message {
    Hello,
}

pub type UserInterface<'ui> = iced_runtime::UserInterface<'ui, Signal, Theme, Renderer>;
pub type Element<'ui> = iced::Element<'ui, Signal, Theme, Renderer>;

pub struct Bar {}

impl Bar {
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
        .clip(false)
    }
}

impl Program for Bar {
    fn view(&self) -> Element<'_> {
        self.logo().into()
        // button("hello")
        //     .on_press(Signal::Message(Message::Hello))
        //     .into()
    }
    fn update(&mut self, message: Message) {
        dbg!(message);
    }
}
