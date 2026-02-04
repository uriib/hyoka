use std::{
    cell::Cell,
    ptr::{self, NonNull},
    time::{Duration, Instant},
};

use futures::{
    SinkExt as _, StreamExt as _,
    channel::mpsc::{Sender, channel},
};
use iced::{
    Color, Font, Pixels, Point, Size, Theme, color,
    font::{Family, Stretch, Style, Weight},
    mouse::Cursor,
    theme::Palette,
};
use iced_core::{layout::Limits, widget::Tree};
use iced_tiny_skia::Renderer;
use rustc_hash::FxHashMap;

use crate::{
    Split, TinyString,
    consumer::{
        programs::{Bar, Program, tooltip},
        window::{Role, Window, WindowManager},
    },
    modules::{
        self, battery,
        clock::{self, Clock},
        dbus::Tray,
        hyprland,
    },
    wayland,
};

const BAR_HEIGHT: u32 = 35;

#[derive(Debug)]
enum Event {
    Wayland(wayland::Event),
    App(AppEvent),
}

#[derive(Debug)]
pub enum AppEvent {
    Hyprland(hyprland::Event),
    Battery(battery::Event),
    Clock(clock::Clock),
    Tray(modules::dbus::TrayEvent),
}

#[derive(Debug, Clone)]
pub enum Action {
    Workspace { id: u8 },
    WindowInfo,
    Battery(TinyString),
    TrayTooltip(Tray),
    TrayAction(Tray),
    CloseTooltip,
}

pub struct Consumer {
    pub wayland: wayland::Client,
    pub display: NonNull<wayland::ffi::wl_display>,
    pub hyprland: Option<hyprland::Client>,
}

impl Consumer {
    pub async fn run(self) {
        let Self {
            wayland:
                wayland::Client {
                    proxy: wayland_proxy,
                    events: mut wayland_events,
                },
            display,
            hyprland,
        } = self;
        let (mut notifier, mut receiver) = channel(4);

        let mut sender = notifier.clone();
        let wayland = async move {
            loop {
                sender
                    .send(Event::Wayland(wayland_events.next().await.unwrap()))
                    .await
                    .unwrap();
            }
        };

        let (hyprctl, hyprland_events) = hyprland
            .map(|hyprland::Client { context, events }| (context, events))
            .split();

        let mut sender = notifier.clone();
        let hyprland = async move {
            if let Some(mut events) = hyprland_events {
                while let Ok(e) = events.try_next() {
                    sender
                        .feed(Event::App(AppEvent::Hyprland(e.unwrap())))
                        .await
                        .unwrap();
                }
                loop {
                    sender
                        .send(Event::App(AppEvent::Hyprland(events.next().await.unwrap())))
                        .await
                        .unwrap();
                }
            }
        };

        let events = notifier.clone();
        let battery_serve = async move {
            let mut sender = events.clone();
            if let Some((e, name, uevent)) = battery::init().unwrap() {
                sender.feed(Event::App(AppEvent::Battery(e))).await.unwrap();
                let mut listener = battery::Listener::new(name).unwrap();

                let listen = async {
                    listener
                        .listen(async |e| {
                            sender.send(Event::App(AppEvent::Battery(e))).await.unwrap()
                        })
                        .await
                };

                let mut sender = events.clone();
                let poll = async {
                    loop {
                        if let Some(e) = uevent.poll() {
                            sender.send(Event::App(AppEvent::Battery(e))).await.unwrap();
                        }
                        let seconds = match e.status {
                            battery::Status::Full | battery::Status::Charging => 3,
                            _ => 6,
                        };
                        compio::time::sleep(Duration::from_secs(seconds)).await;
                    }
                };

                std::future::join!(listen, poll).await;
            }
        };

        let mut sender = notifier.clone();
        let clock = async move {
            let interval = Duration::from_secs(1);
            let mut timer = compio::time::interval_at(Instant::now() + interval, interval);
            loop {
                timer.tick().await;
                sender
                    .send(Event::App(AppEvent::Clock(Clock::now())))
                    .await
                    .unwrap();
            }
        };

        let sender = notifier.clone();
        let (dbus_daemon, dbus_client) = modules::dbus::new(Dispatcher(sender)).await.split();
        let dbus = async {
            if let Some(daemon) = dbus_daemon {
                daemon.serve().await;
            }
        };

        notifier.flush().await.unwrap();
        let mut runner = Runner::new(wayland_proxy, display, hyprctl, dbus_client);
        let consumer = async move {
            loop {
                // TODO: dispatch all pending events at once
                match receiver.next().await.unwrap() {
                    Event::Wayland(event) => {
                        runner.dispatch_wayland(event).await;
                    }
                    Event::App(event) => runner.dispatch_app_event(event),
                }
            }
        };

        std::future::join!(wayland, consumer, hyprland, battery_serve, clock, dbus).await;
    }
}

type Callbacks = FxHashMap<wayland::Callback, Box<dyn FnOnce(&mut Runner)>>;

#[derive(Clone)]
struct Dispatcher(Sender<Event>);
impl modules::dbus::Dispatcher for Dispatcher {
    async fn dispatch(&mut self, e: impl Into<modules::dbus::Event>) {
        match e.into() {
            modules::dbus::Event::Tray(tray_event) => self
                .0
                .send(Event::App(AppEvent::Tray(tray_event)))
                .await
                .unwrap(),
        }
    }
}

struct Runner {
    wayland: wayland::Proxy,
    hyprctl: Option<hyprland::Context>,
    dbus: Option<modules::dbus::Client<Dispatcher>>,

    window_manager: WindowManager,
    display: NonNull<wayland::ffi::wl_display>,
    pointer: NonNull<wayland::ffi::wl_pointer>,
    cursor_shape_device: NonNull<wayland::ffi::wp_cursor_shape_device_v1>,

    theme: Theme,
    callbacks: Callbacks,

    tooltip: Option<Window>,
}

impl Runner {
    fn new(
        mut wayland: wayland::Proxy,
        display: NonNull<wayland::ffi::wl_display>,
        hyprctl: Option<hyprland::Context>,
        dbus: Option<modules::dbus::Client<Dispatcher>>,
    ) -> Self {
        let mut window_manager = WindowManager::default();
        let surface =
            unsafe { wayland::ffi::wl_compositor_create_surface(wayland.globals.compositer()) };
        unsafe {
            wayland::ffi::wl_surface_add_listener(
                surface,
                &wayland::SURFACE_LISTENER,
                &raw mut *wayland.notifier as _,
            );
        };
        let layer_surface = unsafe {
            wayland::ffi::zwlr_layer_shell_v1_get_layer_surface(
                wayland.globals.layer_shell(),
                surface,
                ptr::null_mut(),
                wayland::ffi::ZWLR_LAYER_SHELL_V1_LAYER_TOP,
                c"hyoka".as_ptr(),
            )
        };
        unsafe {
            wayland::ffi::zwlr_layer_surface_v1_add_listener(
                layer_surface,
                &wayland::LAYER_SURFACE_LISTENER,
                &raw mut *wayland.notifier as _,
            );
            wayland::ffi::zwlr_layer_surface_v1_set_size(layer_surface, 0, BAR_HEIGHT);
            wayland::ffi::zwlr_layer_surface_v1_set_anchor(
                layer_surface,
                wayland::ffi::ZWLR_LAYER_SURFACE_V1_ANCHOR_TOP
                    | wayland::ffi::ZWLR_LAYER_SURFACE_V1_ANCHOR_LEFT
                    | wayland::ffi::ZWLR_LAYER_SURFACE_V1_ANCHOR_RIGHT,
            );
            wayland::ffi::zwlr_layer_surface_v1_set_exclusive_zone(layer_surface, 35);
            wayland::ffi::wl_surface_commit(surface);

            wayland::ffi::wl_display_flush(display.as_ptr());
        }
        window_manager.create_window(
            NonNull::new(surface).unwrap(),
            Role::Layer {
                layer_surface: NonNull::new(layer_surface).unwrap(),
            },
            renderer(),
            Box::new(Bar::new()),
            true,
        );

        let pointer = unsafe { wayland::ffi::wl_seat_get_pointer(wayland.globals.seat()) };
        unsafe {
            wayland::ffi::wl_pointer_add_listener(
                pointer,
                &wayland::POINTER_LISTENERL,
                &raw mut *wayland.notifier as _,
            )
        };
        let cursor_shape_device = unsafe {
            wayland::ffi::wp_cursor_shape_manager_v1_get_pointer(
                wayland.globals.cursor_shape_manager(),
                pointer,
            )
        };

        Self {
            wayland,
            display,
            hyprctl,
            dbus,
            tooltip: None,
            window_manager,
            theme: theme(),
            pointer: NonNull::new(pointer).unwrap(),
            cursor_shape_device: NonNull::new(cursor_shape_device).unwrap(),
            callbacks: Default::default(),
        }
    }
    async fn dispatch_wayland(&mut self, event: wayland::Event) -> Option<()> {
        match event {
            wayland::Event::Resize { object, size } => {
                self.window_manager
                    .find_by_object(object)?
                    .clone()
                    .resize(size, self);
            }
            wayland::Event::Rescale { surface, factor } => {
                self.window_manager
                    .find_by_object(surface)?
                    .clone()
                    .rescale(factor, self);
            }
            wayland::Event::Enter { surface, serial } => {
                let win = self.window_manager.find_by_object(surface)?.clone();
                win.mouse(iced::mouse::Event::CursorEntered, self).await;
                win.enter(serial);
                self.window_manager.focused = Some(surface);
            }
            wayland::Event::Mouse(event) => {
                let window = self.window_manager.focused()?;
                window.clone().mouse(event, self).await;
                match event {
                    iced::mouse::Event::CursorLeft => {
                        self.window_manager.focused.take();
                    }
                    _ => (),
                }
            }
            wayland::Event::CallbackDone(cb) => self.callbacks.remove(&cb).unwrap()(self),
        }
        Some(())
    }
    fn dispatch_app_event(&mut self, event: AppEvent) {
        for window in &self.window_manager.subscribers {
            let state = &mut *window.state.borrow_mut();
            state.program.dispatch(&event);
            match &window.surface().role {
                Role::Layer { .. } => {}
                Role::Popup { size, .. } => {
                    let new_size = {
                        let mut view = state.program.view();
                        let mut tree = Tree::new(&view);
                        let node = view.as_widget_mut().layout(
                            &mut tree,
                            &mut state.renderer,
                            &Limits::new(Size::ZERO, Size::INFINITE),
                        );
                        node.bounds().size()
                    };
                    let size = size.replace(new_size);
                    if size.width < new_size.width || size.height < new_size.height {
                        state.resize(
                            [new_size.width, new_size.height].map(|x| x as _),
                            window.surface().surface,
                            &self.wayland.globals,
                        );
                    }
                }
            }
            window.request_redraw(
                window.surface().surface,
                &mut self.wayland.notifier,
                &mut self.callbacks,
            );
        }
        unsafe {
            wayland::ffi::wl_display_flush(self.display.as_ptr());
        }
    }
    async fn action(&mut self, role: &Role, cursor: Cursor, action: Action) -> Option<()> {
        match action {
            Action::Workspace { id } => {
                self.hyprctl
                    .as_mut()?
                    .controller()
                    .await
                    .command(hyprland::Command::Workspace(id))
                    .await;
            }
            Action::WindowInfo => {
                if let Some(win) = self.tooltip.take() {
                    self.window_manager.close_window(win.surface());
                }
                let res = match self
                    .hyprctl
                    .as_mut()?
                    .controller()
                    .await
                    .request(hyprland::Request::ActiveWindow)
                    .await
                {
                    hyprland::Response::Raw(s) => s,
                };
                if let Cursor::Available(Point { x, .. }) = cursor {
                    self.tooltip = self
                        .popup(
                            Box::new(tooltip::WindowInfo::new(res.replace("\t", "        "))),
                            [x as _, BAR_HEIGHT + 1],
                            role,
                            false,
                        )
                        .map(|x| x.clone());
                }
            }
            Action::Battery(init) => {
                if let Some(win) = self.tooltip.take() {
                    self.window_manager.close_window(win.surface());
                };
                if let Cursor::Available(Point { x, .. }) = cursor {
                    self.tooltip = self
                        .popup(
                            Box::new(tooltip::Battery { content: init }),
                            [x as _, BAR_HEIGHT + 1],
                            role,
                            true,
                        )
                        .map(|x| x.clone());
                }
            }
            Action::TrayTooltip(service) => {
                if let Some(win) = self.tooltip.take() {
                    self.window_manager.close_window(win.surface());
                }
                if let Cursor::Available(Point { x, .. }) = cursor {
                    let content =
                        TinyString::from_string(self.dbus.as_mut()?.tray_tooltip(service).await?);
                    self.tooltip = self
                        .popup(
                            Box::new(tooltip::Tooltip { content }),
                            [x as _, BAR_HEIGHT + 1],
                            role,
                            true,
                        )
                        .map(|x| x.clone());
                }
            }
            Action::CloseTooltip => {
                if let Some(win) = self.tooltip.take() {
                    self.window_manager.close_window(win.surface());
                }
            }
            Action::TrayAction(service) => self.dbus.as_mut()?.tray_action(service).await,
        }
        Some(())
    }
    fn popup(
        &mut self,
        program: Box<dyn Program>,
        [x, y]: [u32; 2],
        parent: &Role,
        subscribe: bool,
    ) -> Option<&Window> {
        let mut renderer = renderer();
        let Size { width, height } = {
            let mut view = program.view();
            let mut tree = Tree::new(&view);
            let node = view.as_widget_mut().layout(
                &mut tree,
                &mut renderer,
                &Limits::new(Size::ZERO, Size::INFINITE),
            );
            node.bounds().size()
        };
        let size = width * height;
        if size == 0.0 {
            return None;
        }
        if size == f32::INFINITY {
            log::error!("window has infinity size");
            return None;
        }

        let surface = unsafe {
            wayland::ffi::wl_compositor_create_surface(self.wayland.globals.compositer())
        };
        unsafe {
            wayland::ffi::wl_surface_add_listener(
                surface,
                &wayland::SURFACE_LISTENER,
                &raw mut *self.wayland.notifier as _,
            );
        };
        let xdg_surface = unsafe {
            wayland::ffi::xdg_wm_base_get_xdg_surface(self.wayland.globals.wm_base(), surface)
        };
        unsafe {
            wayland::ffi::xdg_surface_add_listener(
                xdg_surface,
                &wayland::XDG_SURFACE_LISTENER,
                ptr::null_mut(),
            );
        }
        let positioner =
            unsafe { wayland::ffi::xdg_wm_base_create_positioner(self.wayland.globals.wm_base()) };

        unsafe {
            wayland::ffi::xdg_positioner_set_size(positioner, width as _, height as _);
            wayland::ffi::xdg_positioner_set_anchor_rect(positioner, x as _, y as _, 1, 1);
            wayland::ffi::xdg_positioner_set_anchor(
                positioner,
                wayland::ffi::XDG_POSITIONER_ANCHOR_BOTTOM,
            );
            wayland::ffi::xdg_positioner_set_gravity(
                positioner,
                wayland::ffi::XDG_POSITIONER_GRAVITY_BOTTOM,
            );
            wayland::ffi::xdg_positioner_set_constraint_adjustment(
                positioner,
                wayland::ffi::XDG_POSITIONER_CONSTRAINT_ADJUSTMENT_SLIDE_X
                    | wayland::ffi::XDG_POSITIONER_CONSTRAINT_ADJUSTMENT_SLIDE_Y,
            );
        }

        let popup = unsafe {
            match parent {
                Role::Layer { layer_surface } => {
                    let popup = wayland::ffi::xdg_surface_get_popup(
                        xdg_surface,
                        ptr::null_mut(),
                        positioner,
                    );
                    wayland::ffi::zwlr_layer_surface_v1_get_popup(layer_surface.as_ptr(), popup);
                    popup
                }
                Role::Popup {
                    xdg_surface: parent,
                    ..
                } => wayland::ffi::xdg_surface_get_popup(xdg_surface, parent.as_ptr(), positioner),
            }
        };
        unsafe {
            wayland::ffi::xdg_popup_add_listener(
                popup,
                &wayland::XDG_POPUP_LISTENER,
                &raw mut *self.wayland.notifier as _,
            );
        };
        unsafe {
            wayland::ffi::wl_surface_commit(surface);
            wayland::ffi::wl_display_flush(self.display.as_ptr());
        };
        let surface = NonNull::new(surface).unwrap();
        let win = self.window_manager.create_window(
            surface,
            Role::Popup {
                xdg_surface: NonNull::new(xdg_surface).unwrap(),
                popup: NonNull::new(popup).unwrap(),
                positioner: NonNull::new(positioner).unwrap(),
                size: Cell::new(Size::new(width, height)),
            },
            renderer,
            program,
            subscribe,
        );
        Some(win)
    }
}

const BACKGROUND: Color = Color::from_rgba8(30, 28, 34, 0.38);

const PURPLE: Color = color!(0xa476f7);
const WHITE: Color = color!(0xcdd6f5);
const GREEN: Color = color!(0x92b673);
const YELLOW: Color = color!(0xe09733);
const RED: Color = color!(0xf25b4f);

pub fn theme() -> Theme {
    Theme::custom(
        "paper dark",
        Palette {
            background: BACKGROUND,
            text: WHITE,
            primary: PURPLE,
            success: GREEN,
            warning: YELLOW,
            danger: RED,
        },
    )
}

pub fn renderer() -> iced_tiny_skia::Renderer {
    Renderer::new(
        Font {
            family: Family::Name("SF Pro Display"),
            weight: Weight::Normal,
            stretch: Stretch::Normal,
            style: Style::Normal,
        },
        Pixels(15.5),
    )
}

trait StyleSheet {
    fn css_injection(&self) -> String;
}
// foreground’, ‘success’, ‘warning’, ‘error’, ‘accent’
impl StyleSheet for Theme {
    fn css_injection(&self) -> String {
        let Palette {
            text,
            primary,
            success,
            warning,
            danger,
            ..
        } = self.palette();
        format!(
            concat!(
                "* {{ fill:{} }}",
                ".foreground {{ fill:{} }}",
                ".success {{ fill:{} }}",
                ".warning {{ fill:{} }}",
                ".error {{ fill:{} }}",
                ".accent {{ fill:{} }}",
            ),
            text, text, success, warning, danger, primary
        )
    }
}

mod programs;
mod window;
