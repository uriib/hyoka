use std::{
    cell::RefCell,
    ffi::c_void,
    fmt, mem,
    ops::Deref,
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
        programs::{Program, Signal, UserInterface},
    },
    wayland::{self, Callback},
};

pub struct Layer {
    pub layer_surface: NonNull<wayland::ffi::zwlr_layer_surface_v1>,
}

impl Drop for Layer {
    fn drop(&mut self) {
        unsafe { wayland::ffi::zwlr_layer_surface_v1_destroy(self.layer_surface.as_ptr()) };
    }
}

pub enum Role {
    Layer(Layer),
}

impl Role {
    pub fn key(&self, mut cb: impl FnMut(NonNull<c_void>)) {
        match self {
            &Role::Layer(Layer { layer_surface }) => cb(unsafe { mem::transmute(layer_surface) }),
        }
    }
}

pub type Surface = NonNull<wayland::ffi::wl_surface>;

enum ConfigState<'ui> {
    Configured {
        buffer: Buffer,
        ui: UserInterface<'ui>,
        clip_mask: tiny_skia::Mask,
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

pub struct Inner {
    role: Role,
    surface: Surface,

    cursor: Cursor,
    serial: Option<u32>,
    shape: Option<Interaction>,
    config_state: ConfigState<'static>,

    program: Box<dyn Program>,
}

impl Inner {
    fn redraw(
        &mut self,
        renderer: &mut Renderer,
        theme: &Theme,
        display: NonNull<wayland::ffi::wl_display>,
    ) {
        if let ConfigState::Configured {
            ui,
            buffer,
            clip_mask,
            last_layers,
        } = &mut self.config_state
        {
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

            let layers = renderer.layers();
            let [width, height] = buffer.viewport.surface_size.map(|x| x as _);
            let bounds = Rectangle::with_size(Size { width, height });
            let damage = if let Some(last_layers) = last_layers {
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

            renderer.draw(
                &mut buffer.pixels(),
                clip_mask,
                &buffer.viewport.to_iced_viewport(),
                &damage,
                theme.palette().background,
            );

            unsafe {
                wayland::ffi::wl_surface_attach(
                    self.surface.as_ptr(),
                    buffer.buffer.as_ptr(),
                    0,
                    0,
                );
                for rect in damage {
                    wayland::ffi::wl_surface_damage(
                        self.surface.as_ptr(),
                        rect.x as _,
                        rect.y as _,
                        rect.width.ceil() as _,
                        rect.height.ceil() as _,
                    );
                }
                wayland::ffi::wl_surface_commit(self.surface.as_ptr());
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
    pub focused: Option<Window>,
}

impl WindowManager {
    pub fn create_window(
        &mut self,
        surface: Surface,
        role: Role,
        program: Box<dyn Program>,
    ) -> &mut Window {
        let window = Window::new(surface, role, program);
        window.borrow().role.key(|k| {
            self.lut.try_insert(k, window.clone()).unwrap();
        });
        self.lut
            .try_insert(unsafe { mem::transmute(surface) }, window)
            .unwrap()
    }
    pub fn close_window(&mut self, window: impl Deref<Target = Inner>) {
        window.role.key(|k| {
            self.lut.remove(&k).unwrap();
        });
        self.lut.remove(&unsafe { mem::transmute(window.surface) });
    }
    pub fn find_by_object<T>(&self, obj: NonNull<T>) -> &Window {
        self.lut.get(&unsafe { mem::transmute(obj) }).unwrap()
    }
}

impl Window {
    fn new(surface: Surface, role: Role, program: Box<dyn Program>) -> Self {
        let inner = Inner {
            role,
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
                self.request_redraw(window.surface, runner);
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
    }

    fn request_redraw(&self, surface: Surface, runner: &mut Runner) {
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

        let surface = window.surface;
        unsafe { wayland::ffi::wl_surface_set_buffer_scale(surface.as_ptr(), scale as _) };
        self.request_redraw(surface, runner);
        // self.redraw(&mut runner.renderer, &runner.theme);
        // unsafe { wayland::ffi::wl_display_flush(runner.display.as_ptr()) };
    }

    pub fn enter(&self, serial: u32) {
        self.borrow_mut().serial = Some(serial)
    }

    pub fn mouse(&self, event: mouse::Event, runner: &mut Runner) {
        if let Inner {
            surface,
            ref mut cursor,
            ref mut serial,
            ref mut shape,
            config_state: ConfigState::Configured { ref mut ui, .. },
            ref mut program,
            ..
        } = *self.borrow_mut()
        {
            match event {
                mouse::Event::CursorMoved { position } => {
                    *cursor = Cursor::Available(position);
                }
                mouse::Event::CursorLeft => {
                    *cursor = Cursor::Unavailable;
                    *serial = None;
                    *shape = None;
                }
                _ => {}
            }
            let mut messages = vec![];
            let (state, _) = ui.update(
                &[iced::Event::Mouse(event)],
                *cursor,
                &mut runner.renderer,
                &mut Clipboard,
                &mut messages,
            );
            for message in messages {
                match message {
                    Signal::Message(message) => program.update(message),
                    Signal::Action(action) => action(runner),
                }
            }
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
        }
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
