use std::{
    cell::RefCell,
    ffi::c_void,
    fmt, mem,
    ops::{Deref, Index},
    os::fd::AsRawFd as _,
    ptr::{self, NonNull},
    rc::Rc,
    time::Instant,
};

use iced::{
    Rectangle, Size, Theme,
    advanced::{clipboard, renderer::Style},
    mouse::{self, Cursor, Interaction},
    window::RedrawRequest,
};
use iced_core::widget::Operation;
use iced_runtime::user_interface::Cache;
use iced_tiny_skia::{Renderer, graphics};
use rustc_hash::FxHashMap;
use rustix::{
    fs::MemfdFlags,
    mm::{MapFlags, ProtFlags},
};
use tiny_skia::{Mask, PixmapMut};

use crate::{
    consumer::{
        Runner,
        programs::{Element, Program, Signal, UserInterface},
    },
    wayland::{self, Callback},
};

#[derive(Clone, Copy)]
pub enum Role {
    Layer {
        layer_surface: NonNull<wayland::ffi::zwlr_layer_surface_v1>,
    },
    Popup {
        xdg_surface: NonNull<wayland::ffi::xdg_surface>,
        popup: NonNull<wayland::ffi::xdg_popup>,
        positioner: NonNull<wayland::ffi::xdg_positioner>,
    },
}

impl Role {
    fn destroy(&self) {
        match self {
            Role::Layer { layer_surface } => unsafe {
                wayland::ffi::zwlr_layer_surface_v1_destroy(layer_surface.as_ptr())
            },
            Role::Popup {
                xdg_surface,
                popup,
                positioner,
            } => unsafe {
                wayland::ffi::xdg_popup_destroy(popup.as_ptr());
                wayland::ffi::xdg_surface_destroy(xdg_surface.as_ptr());
                wayland::ffi::xdg_positioner_destroy(positioner.as_ptr());
            },
        }
    }
    fn key(&self, mut cb: impl FnMut(NonNull<c_void>)) {
        match self {
            &Role::Layer { layer_surface } => cb(unsafe { mem::transmute(layer_surface) }),
            &Role::Popup { popup, .. } => cb(unsafe { mem::transmute(popup) }),
        }
    }
}

pub type WlSurface = NonNull<wayland::ffi::wl_surface>;

enum ConfigState<'ui> {
    Configured {
        buffer: Buffer,
        clip_mask: tiny_skia::Mask,
        ui: UserInterface<'ui>,
        last_layers: Option<Vec<iced_tiny_skia::Layer>>,
    },
    Unconfigured {
        scale_factor: u32,
    },
}

impl ConfigState<'_> {
    fn forget_lifetime(self) -> ConfigState<'static> {
        unsafe { mem::transmute(self) }
    }
}
impl Default for ConfigState<'_> {
    fn default() -> Self {
        Self::Unconfigured { scale_factor: 1 }
    }
}

pub struct Surface {
    pub role: Role,
    pub surface: WlSurface,
}

impl Drop for Surface {
    fn drop(&mut self) {
        self.role.destroy();
        unsafe { wayland::ffi::wl_surface_destroy(self.surface.as_ptr()) };
    }
}

pub struct Inner {
    surface: Surface,

    cursor: Cursor,
    serial: Option<u32>,
    shape: Option<Interaction>,
    config_state: ConfigState<'static>,

    program: Box<dyn Program>,
}

fn rebuild_ui<'ui>(
    ptr: *mut UserInterface<'ui>,
    root: Element<'ui>,
    bounds: Size,
    renderer: &mut Renderer,
) {
    let ui = unsafe { ptr.read() };
    let ui = UserInterface::build(root, bounds, ui.into_cache(), renderer);
    let ui = unsafe { ptr.replace(ui) };
    mem::forget(ui);
}

impl Inner {
    pub fn program(&mut self) -> &mut dyn Program {
        &mut *self.program
    }
    // pub fn cursor(&self) -> Cursor {
    //     self.cursor
    // }
    pub fn surface(&self) -> &Surface {
        &self.surface
    }
    pub fn rebuild(&mut self, renderer: &mut Renderer) {
        let Inner {
            config_state,
            program,
            ..
        } = self;

        if let ConfigState::Configured { ui, buffer, .. } = config_state {
            let [width, height] = buffer.viewport.surface_size;
            rebuild_ui(
                ui as _,
                unsafe { mem::transmute(program.view()) },
                Size::new(width as _, height as _),
                renderer,
            )
        }
    }
    fn redraw(
        &mut self,
        renderer: &mut Renderer,
        theme: &Theme,
        display: NonNull<wayland::ffi::wl_display>,
    ) {
        if let ConfigState::Configured {
            buffer,
            clip_mask,
            last_layers,
            ui,
        } = &mut self.config_state
        {
            let [width, height] = buffer.viewport.surface_size;
            rebuild_ui(
                ui,
                unsafe { mem::transmute(self.program.view()) },
                Size::new(width as _, height as _),
                renderer,
            );
            // let mut ui = UserInterface::build(
            //     self.program.view(),
            //     Size::new(width as _, height as _),
            //     mem::take(&mut self.cache),
            //     renderer,
            // );
            ui.update(
                &[iced::Event::Window(iced::window::Event::RedrawRequested(
                    Instant::now(),
                ))],
                self.cursor,
                renderer,
                &mut Clipboard,
                &mut vec![],
            );
            ui.draw(
                renderer,
                theme,
                &Style {
                    text_color: theme.palette().text,
                },
                self.cursor,
            );
            // self.cache = ui.into_cache();

            let layers = renderer.layers();
            let [width, height] = buffer.viewport.surface_size.map(|x| x as _);
            let bounds = Rectangle::with_size(Size { width, height });
            let mut damage = if let Some(last_layers) = last_layers {
                graphics::damage::diff(
                    &last_layers,
                    layers,
                    |layer| vec![layer.bounds],
                    iced_tiny_skia::Layer::damage,
                )
            } else {
                vec![bounds]
            };
            *last_layers = Some(layers.to_vec());

            for rect in &mut damage {
                rect.width = rect.width.ceil();
                rect.height = rect.height.ceil();
            }

            renderer.draw(
                &mut buffer.pixels(),
                clip_mask,
                &buffer.viewport.to_iced_viewport(),
                &damage,
                self.program.background(theme),
            );

            let surface = self.surface.surface.as_ptr();
            unsafe {
                wayland::ffi::wl_surface_attach(surface, buffer.buffer.as_ptr(), 0, 0);
                // let [width, height] = buffer.viewport.buffer_size().map(|x| x as _);
                // wayland::ffi::wl_surface_damage_buffer(surface, 0, 0, width, height);
                for rect in damage {
                    wayland::ffi::wl_surface_damage(
                        surface,
                        rect.x as _,
                        rect.y as _,
                        rect.width as _,
                        rect.height as _,
                    );
                }
                wayland::ffi::wl_surface_commit(surface);
                wayland::ffi::wl_display_flush(display.as_ptr());
            }
        }
    }
}

#[derive(Clone)]
pub struct Window(Rc<RefCell<Inner>>);

// impl Window {
//     fn new(inner: Inner) -> Self {
//         Self(Rc::new(RefCell::new(inner)))
//     }
// }

impl Deref for Window {
    type Target = Rc<RefCell<Inner>>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Debug for Window {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.as_ptr())
    }
}

// #[derive(Default)]
// struct Flag {
//     value: bool,
// }
//
// impl Flag {
//     fn when_set<F: FnOnce()>(&mut self, f: F) {
//         if self.value {
//             f();
//         }
//         self.value = false;
//     }
// }

#[derive(Default)]
pub struct WindowManager {
    lut: FxHashMap<NonNull<c_void>, Window>,
    pub focused: Option<WlSurface>,
}

impl WindowManager {
    pub fn create_window(
        &mut self,
        surface: WlSurface,
        role: Role,
        program: Box<dyn Program>,
    ) -> &mut Window {
        let window = Window::new(Surface { surface, role }, program);
        window.borrow().surface.role.key(|k| {
            self.lut.try_insert(k, window.clone()).unwrap();
        });
        self.lut.try_insert(surface.cast(), window).unwrap()
    }
    pub fn close_window(&mut self, window: impl Deref<Target = Inner>) {
        if Some(window.surface.surface) == self.focused {
            self.focused.take();
        }

        window.surface.role.key(|k| {
            self.lut.remove(&k).unwrap();
        });
        self.lut.remove(&window.surface.surface.cast());
    }
    pub fn find_by_object<T>(&self, obj: NonNull<T>) -> Option<&Window> {
        self.lut.get(&obj.cast())
    }
    pub fn focused(&self) -> Option<&Window> {
        self.lut.get(&self.focused?.cast())
    }
}

impl<T> Index<NonNull<T>> for WindowManager {
    type Output = Window;
    fn index(&self, obj: NonNull<T>) -> &Self::Output {
        self.lut.get(&obj.cast()).unwrap()
    }
}

impl Window {
    fn new(surface: Surface, program: Box<dyn Program>) -> Self {
        let inner = Inner {
            surface,
            cursor: Cursor::Unavailable,
            serial: None,
            shape: None,
            config_state: ConfigState::default(),
            program,
        };
        Self(Rc::new(RefCell::new(inner)))
    }
    pub fn resize(&mut self, size @ [width, height]: [u32; 2], runner: &mut Runner) {
        let mut window = self.borrow_mut();
        match mem::take(&mut window.config_state) {
            ConfigState::Configured { buffer, ui, .. } => {
                let viewport = buffer.viewport.with_surface_size(size);
                window.config_state = ConfigState::Configured {
                    buffer: runner.wayland.globals.create_buffer(viewport),
                    ui: ui.relayout(Size::new(width as _, height as _), &mut runner.renderer),
                    clip_mask: Mask::new(width, height).unwrap(),
                    last_layers: None,
                };
                self.request_redraw(window.surface.surface, runner);
            }
            ConfigState::Unconfigured { scale_factor } => {
                let viewport = Viewport {
                    surface_size: size,
                    buffer_scale: scale_factor,
                    _buffer_transform: wayland::ffi::WL_OUTPUT_TRANSFORM_NORMAL,
                };
                window.config_state = ConfigState::Configured {
                    buffer: runner.wayland.globals.create_buffer(viewport),
                    ui: iced_runtime::UserInterface::build(
                        window.program.view(),
                        Size::new(width as _, height as _),
                        Cache::new(),
                        &mut runner.renderer,
                    ),
                    clip_mask: Mask::new(width, height).unwrap(),
                    last_layers: None,
                }
                .forget_lifetime();
                window.redraw(&mut runner.renderer, &runner.theme, runner.display);
            }
        }

        unsafe { wayland::ffi::wl_display_flush(runner.display.as_ptr()) };
    }

    pub fn rescale(&self, scale: u32, runner: &mut Runner) {
        let mut window = self.borrow_mut();
        match &mut window.config_state {
            ConfigState::Configured {
                buffer,
                clip_mask,
                last_layers,
                ..
            } => {
                // for hyprland sends scale event when scale does not change
                if buffer.viewport.buffer_scale == scale {
                    return;
                }
                *buffer = runner
                    .wayland
                    .globals
                    .create_buffer(buffer.viewport.with_buffer_scale(scale));

                let [width, height] = buffer.viewport.buffer_size();
                *clip_mask = Mask::new(width, height).unwrap();
                *last_layers = None;
            }
            ConfigState::Unconfigured { scale_factor } => {
                *scale_factor = scale;
            }
        }

        let surface = window.surface.surface;
        unsafe { wayland::ffi::wl_surface_set_buffer_scale(surface.as_ptr(), scale as _) };
        self.request_redraw(surface, runner);
        unsafe { wayland::ffi::wl_display_flush(runner.display.as_ptr()) };
    }

    pub fn request_redraw(&self, surface: WlSurface, runner: &mut Runner) {
        let callback = unsafe { wayland::ffi::wl_surface_frame(surface.as_ptr()) };
        unsafe {
            wayland::ffi::wl_callback_add_listener(
                callback,
                &wayland::CALLBACK_LISTENER,
                &raw mut *runner.wayland.notifier as _,
            )
        };
        unsafe {
            wayland::ffi::wl_surface_commit(surface.as_ptr());
        };
        runner.register_callback(Callback::from_raw(callback), {
            let window = self.clone();
            move |runner| {
                window
                    .borrow_mut()
                    .redraw(&mut runner.renderer, &runner.theme, runner.display)
            }
        });
    }

    pub fn enter(&self, serial: u32) {
        self.borrow_mut().serial = Some(serial)
    }

    pub async fn mouse(&self, event: mouse::Event, runner: &mut Runner) {
        let Inner {
            surface: Surface { surface, role },
            ref mut cursor,
            ref mut serial,
            ref mut shape,
            ref mut config_state,
            ref mut program,
            ..
        } = *self.borrow_mut();

        if let ConfigState::Configured { ui, .. } = config_state {
            match event {
                mouse::Event::CursorMoved { position } => {
                    *cursor = Cursor::Available(position);
                }
                mouse::Event::CursorLeft => {
                    eprintln!("leave");
                    ui.operate(&mut runner.renderer, &mut Leave);
                    *cursor = Cursor::Unavailable;
                    *serial = None;
                    *shape = None;
                }
                _ => {}
            }
            let mut messages = vec![];
            let state;
            {
                // let mut ui = UserInterface::build(
                //     program.view(),
                //     Size::new(width as _, height as _),
                //     mem::take(cache),
                //     &mut runner.renderer,
                // );
                (state, _) = ui.update(
                    &[iced::Event::Mouse(event)],
                    *cursor,
                    &mut runner.renderer,
                    &mut Clipboard,
                    &mut messages,
                );
            }
            for message in messages {
                match message {
                    Signal::Message(message) => program.update(message),
                    Signal::Action(action) => {
                        dbg!(&action);
                        runner.action(role, *cursor, action).await;
                        self.request_redraw(surface, runner)
                    }
                }
            }
            // dbg!(&state);
            match state {
                iced_runtime::user_interface::State::Outdated => {
                    eprintln!("outdated");
                }
                iced_runtime::user_interface::State::Updated {
                    mouse_interaction,
                    redraw_request,
                    ..
                } => {
                    if let RedrawRequest::NextFrame = redraw_request {
                        self.request_redraw(surface, runner)
                    }
                    if let Some(serial) = *serial
                        && Some(mouse_interaction) != *shape
                    {
                        match cursor_shape(mouse_interaction) {
                            CursorShape::Shape(shape) => unsafe {
                                wayland::ffi::wp_cursor_shape_device_v1_set_shape(
                                    runner.cursor_shape_device.as_ptr(),
                                    serial,
                                    shape,
                                )
                            },
                            CursorShape::Hide => unsafe {
                                wayland::ffi::wl_pointer_set_cursor(
                                    runner.pointer.as_ptr(),
                                    serial,
                                    ptr::null_mut(),
                                    0,
                                    0,
                                );
                            },
                        }
                    }
                }
            }
            unsafe { wayland::ffi::wl_display_flush(runner.display.as_ptr()) };
        }
    }
}

struct Leave;

impl Operation for Leave {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<()>)) {
        operate(self);
    }
    fn focusable(
        &mut self,
        _id: Option<&iced::widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn iced_core::widget::operation::Focusable,
    ) {
        state.unfocus();
    }
}

struct Buffer {
    pub buffer: NonNull<wayland::ffi::wl_buffer>,
    pub viewport: Viewport,
    pub ptr: NonNull<u8>,
}

impl Buffer {
    fn pixels(&self) -> tiny_skia::PixmapMut<'_> {
        PixmapMut::from_bytes(
            self.data(),
            self.viewport.buffer_width(),
            self.viewport.buffer_height(),
        )
        .unwrap()
    }
    fn data(&self) -> &mut [u8] {
        unsafe {
            std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.viewport.buffer_byte_size())
        }
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        unsafe {
            wayland::ffi::wl_buffer_destroy(self.buffer.as_ptr());
            rustix::mm::munmap(self.ptr.as_ptr() as _, self.viewport.buffer_byte_size()).unwrap();
        }
    }
}

impl wayland::Globals {
    fn create_buffer(&self, viewport: Viewport) -> Buffer {
        let len = viewport.buffer_byte_size();
        let fd = rustix::fs::memfd_create(c"", MemfdFlags::empty()).unwrap();
        rustix::fs::ftruncate(&fd, len as _).unwrap();

        let ptr = unsafe {
            rustix::mm::mmap(
                ptr::null_mut(),
                len,
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::SHARED,
                &fd,
                0,
            )
        }
        .unwrap() as _;

        let pool =
            unsafe { wayland::ffi::wl_shm_create_pool(self.shm(), fd.as_raw_fd(), len as _) };
        let buffer = unsafe {
            wayland::ffi::wl_shm_pool_create_buffer(
                pool,
                0,
                viewport.buffer_width() as _,
                viewport.buffer_height() as _,
                viewport.buffer_width() as i32 * 4,
                wayland::ffi::WL_SHM_FORMAT_ARGB8888,
            )
        };
        unsafe { wayland::ffi::wl_shm_pool_destroy(pool) };
        Buffer {
            buffer: NonNull::new(buffer).unwrap(),
            viewport,
            ptr: NonNull::new(ptr).unwrap(),
        }
    }
}

struct Clipboard;

impl clipboard::Clipboard for Clipboard {
    fn read(&self, kind: clipboard::Kind) -> Option<String> {
        _ = kind;
        None
    }

    fn write(&mut self, kind: clipboard::Kind, contents: String) {
        _ = (kind, contents);
    }
}

#[derive(Clone, Copy)]
struct Viewport {
    surface_size: [u32; 2],
    buffer_scale: u32,
    _buffer_transform: wayland::ffi::wl_output_transform,
}

impl Viewport {
    fn with_surface_size(self, surface_size: [u32; 2]) -> Self {
        Self {
            surface_size,
            ..self
        }
    }
    fn with_buffer_scale(self, buffer_scale: u32) -> Self {
        Self {
            buffer_scale,
            ..self
        }
    }
    fn buffer_width(&self) -> u32 {
        self.surface_size[0] * self.buffer_scale
    }
    fn buffer_height(&self) -> u32 {
        self.surface_size[1] * self.buffer_scale
    }
    fn buffer_size(&self) -> [u32; 2] {
        self.surface_size.map(|x| x * self.buffer_scale)
    }
    fn buffer_byte_size(&self) -> usize {
        let [width, height] = self.buffer_size().map(|x| x as usize);
        width * height * 4
    }
    fn to_iced_viewport(&self) -> graphics::Viewport {
        let [width, height] = self.buffer_size();
        graphics::Viewport::with_physical_size(Size { width, height }, self.buffer_scale as _)
    }
}

enum CursorShape {
    Shape(wayland::ffi::wp_cursor_shape_device_v1_shape),
    Hide,
}

fn cursor_shape(interaction: Interaction) -> CursorShape {
    match interaction {
        Interaction::None => {
            CursorShape::Shape(wayland::ffi::WP_CURSOR_SHAPE_DEVICE_V1_SHAPE_DEFAULT)
        }
        Interaction::Hidden => CursorShape::Hide,
        Interaction::Idle => todo!(), // what is idle ?
        Interaction::ContextMenu => {
            CursorShape::Shape(wayland::ffi::WP_CURSOR_SHAPE_DEVICE_V1_SHAPE_CONTEXT_MENU)
        }
        Interaction::Help => CursorShape::Shape(wayland::ffi::WP_CURSOR_SHAPE_DEVICE_V1_SHAPE_HELP),
        Interaction::Pointer => {
            CursorShape::Shape(wayland::ffi::WP_CURSOR_SHAPE_DEVICE_V1_SHAPE_POINTER)
        }
        Interaction::Progress => {
            CursorShape::Shape(wayland::ffi::WP_CURSOR_SHAPE_DEVICE_V1_SHAPE_PROGRESS)
        }
        Interaction::Wait => CursorShape::Shape(wayland::ffi::WP_CURSOR_SHAPE_DEVICE_V1_SHAPE_WAIT),
        Interaction::Cell => CursorShape::Shape(wayland::ffi::WP_CURSOR_SHAPE_DEVICE_V1_SHAPE_CELL),
        Interaction::Crosshair => {
            CursorShape::Shape(wayland::ffi::WP_CURSOR_SHAPE_DEVICE_V1_SHAPE_CROSSHAIR)
        }
        Interaction::Text => CursorShape::Shape(wayland::ffi::WP_CURSOR_SHAPE_DEVICE_V1_SHAPE_TEXT),
        Interaction::Alias => {
            CursorShape::Shape(wayland::ffi::WP_CURSOR_SHAPE_DEVICE_V1_SHAPE_ALIAS)
        }
        Interaction::Copy => CursorShape::Shape(wayland::ffi::WP_CURSOR_SHAPE_DEVICE_V1_SHAPE_COPY),
        Interaction::Move => CursorShape::Shape(wayland::ffi::WP_CURSOR_SHAPE_DEVICE_V1_SHAPE_MOVE),
        Interaction::NoDrop => {
            CursorShape::Shape(wayland::ffi::WP_CURSOR_SHAPE_DEVICE_V1_SHAPE_NO_DROP)
        }
        Interaction::NotAllowed => {
            CursorShape::Shape(wayland::ffi::WP_CURSOR_SHAPE_DEVICE_V1_SHAPE_NOT_ALLOWED)
        }
        Interaction::Grab => CursorShape::Shape(wayland::ffi::WP_CURSOR_SHAPE_DEVICE_V1_SHAPE_GRAB),
        Interaction::Grabbing => {
            CursorShape::Shape(wayland::ffi::WP_CURSOR_SHAPE_DEVICE_V1_SHAPE_GRABBING)
        }
        Interaction::ResizingHorizontally => todo!(),
        Interaction::ResizingVertically => todo!(),
        Interaction::ResizingDiagonallyUp => todo!(),
        Interaction::ResizingDiagonallyDown => todo!(),
        Interaction::ResizingColumn => {
            CursorShape::Shape(wayland::ffi::WP_CURSOR_SHAPE_DEVICE_V1_SHAPE_COL_RESIZE)
        }
        Interaction::ResizingRow => {
            CursorShape::Shape(wayland::ffi::WP_CURSOR_SHAPE_DEVICE_V1_SHAPE_ROW_RESIZE)
        }
        Interaction::AllScroll => {
            CursorShape::Shape(wayland::ffi::WP_CURSOR_SHAPE_DEVICE_V1_SHAPE_ALL_SCROLL)
        }
        Interaction::ZoomIn => {
            CursorShape::Shape(wayland::ffi::WP_CURSOR_SHAPE_DEVICE_V1_SHAPE_ZOOM_IN)
        }
        Interaction::ZoomOut => {
            CursorShape::Shape(wayland::ffi::WP_CURSOR_SHAPE_DEVICE_V1_SHAPE_ZOOM_OUT)
        }
    }
}
