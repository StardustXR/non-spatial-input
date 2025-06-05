use crate::wayland::WlHandler;
use as_raw_xcb_connection::{xcb_connection_t, ValidConnection};
use glam::vec2;
use ipc::{send_input_ipc, Message};
use softbuffer::Surface;
use std::process::exit;
use std::{num::NonZeroU32, rc::Rc};
use wayland_client::{backend::Backend, globals::registry_queue_init, protocol::wl_seat};
use winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use winit::{
	dpi::{LogicalPosition, Size},
	event::{
		DeviceEvent, ElementState, Event, KeyEvent, Modifiers, MouseButton, MouseScrollDelta,
		WindowEvent,
	},
	event_loop::{EventLoop, EventLoopWindowTarget},
	keyboard::Key,
	platform::scancode::PhysicalKeyExtScancode,
	raw_window_handle::{WaylandDisplayHandle, XcbDisplayHandle},
	window::{CursorGrabMode, Window, WindowBuilder},
};
use xkbcommon::xkb::{
	ffi::XKB_KEYMAP_FORMAT_TEXT_V1,
	x11::{get_core_keyboard_device_id, keymap_new_from_device},
	Keymap, KEYMAP_COMPILE_NO_FLAGS, KEYMAP_FORMAT_TEXT_V1,
};

pub struct InputWindow {
	window: Rc<Window>,
	surface: Surface<Rc<Window>, Rc<Window>>,
	mouse_delta: Option<LogicalPosition<f64>>,
	grabbed: bool,
	modifiers: Modifiers,
}
impl InputWindow {
	pub fn new(event_loop: &EventLoop<()>) -> Self {
		let size = Size::Logical([400, 300].into());
		let window = Rc::new(
			WindowBuilder::new()
				.with_title("Manifold")
				.with_inner_size(size)
				.with_resizable(false)
				.build(event_loop)
				.unwrap(),
		);

		let xcb_context = xkbcommon::xkb::Context::new(0);
		let keymap = match window.display_handle().map(|handle| handle.as_raw()) {
			Ok(RawDisplayHandle::Wayland(WaylandDisplayHandle { display, .. })) => unsafe {
				let backend = Backend::from_foreign_display(
					display.as_ptr() as *mut wayland_sys::client::wl_display
				);
				let conn = wayland_client::Connection::from_backend(backend);
				let (globals, mut queue) = registry_queue_init::<WlHandler>(&conn).unwrap();
				let qh = queue.handle();
				let _seat: wl_seat::WlSeat = globals.bind(&qh, 7..=8, ()).unwrap();
				let mut wl_handler = WlHandler { keymap: None };
				eprintln!("Waiting for keymap from compositor");
				while wl_handler.keymap.is_none() {
					queue.roundtrip(&mut wl_handler).unwrap();
				}
				Keymap::new_from_string(
					&xcb_context,
					String::from_utf8(wl_handler.keymap.unwrap()).unwrap(),
					KEYMAP_FORMAT_TEXT_V1,
					KEYMAP_COMPILE_NO_FLAGS,
				)
				.unwrap()
			},
			Ok(RawDisplayHandle::Xcb(XcbDisplayHandle {
				connection: Some(conn),
				..
			})) => unsafe {
				keymap_new_from_device(
					&xcb_context,
					ValidConnection::new(conn.as_ptr() as *mut xcb_connection_t),
					get_core_keyboard_device_id(ValidConnection::new(
						conn.as_ptr() as *mut xcb_connection_t
					)),
					KEYMAP_COMPILE_NO_FLAGS,
				)
			},
			_ => Keymap::new_from_names(&xcb_context, "", "", "", "", None, 0).unwrap(),
		};
		send_input_ipc(Message::Keymap(
			keymap.get_as_string(XKB_KEYMAP_FORMAT_TEXT_V1),
		));

		let context = softbuffer::Context::new(window.clone()).unwrap();
		let surface = softbuffer::Surface::new(&context, window.clone()).unwrap();

		let mut input_window = InputWindow {
			window,
			surface,
			mouse_delta: None,
			grabbed: true,
			modifiers: Modifiers::default(),
		};

		input_window.set_grab(false);
		input_window
	}

	pub fn handle_event(&mut self, event: Event<()>, elwt: &EventLoopWindowTarget<()>) {
		match event {
			Event::WindowEvent { window_id, event } if window_id == self.window.id() => match event
			{
				WindowEvent::CloseRequested => elwt.exit(),
				_ => self.handle_window_event(event),
			},
			Event::DeviceEvent {
				event: DeviceEvent::MouseMotion { delta },
				..
			} => {
				self.handle_mouse_delta(delta);
			}
			Event::AboutToWait => {
				self.redraw();
			}
			_ => {}
		}
	}

	fn handle_mouse_delta(&mut self, delta: (f64, f64)) {
		if self.grabbed {
			self.mouse_delta = Some(LogicalPosition::new(delta.0, delta.1));
			send_input_ipc(Message::MouseMove([delta.0 as f32, delta.1 as f32].into()));
		} else {
			self.mouse_delta = None;
		};
	}

	fn handle_window_event(&mut self, event: WindowEvent) {
		match event {
			WindowEvent::MouseInput { state, button, .. } => self.handle_mouse_input(state, button),
			WindowEvent::MouseWheel { delta, .. } => match delta {
				MouseScrollDelta::LineDelta(x, y) => {
					send_input_ipc(Message::MouseAxisContinuous(vec2(x, y).into()))
				}
				MouseScrollDelta::PixelDelta(p) => send_input_ipc(Message::MouseAxisDiscrete(
					vec2(p.x as f32, p.y as f32).into(),
				)),
			},
			WindowEvent::KeyboardInput { event, .. } => self.handle_keyboard_input(event),
			WindowEvent::ModifiersChanged(state) => self.modifiers = state,
			WindowEvent::CursorEntered { .. } => {
				send_input_ipc(Message::ResetInput);
			}
			WindowEvent::CursorLeft { .. } => {
				send_input_ipc(Message::ResetInput);
			}

			WindowEvent::Destroyed => {
				send_input_ipc(Message::ResetInput);
				send_input_ipc(Message::Disconnect);
				exit(0);
			}
			WindowEvent::CloseRequested => {
				send_input_ipc(Message::ResetInput);
				send_input_ipc(Message::Disconnect);
				exit(0);
			}
			WindowEvent::RedrawRequested => {
				self.redraw();
			}
			_ => (),
		}
	}

	fn redraw(&mut self) {
		let delta = self.mouse_delta.unwrap_or_default();
		let window_size = self.window.inner_size();

		self.surface
			.resize(
				NonZeroU32::new(window_size.width).unwrap(),
				NonZeroU32::new(window_size.height).unwrap(),
			)
			.unwrap();
		let mut buffer = self.surface.buffer_mut().unwrap();

		// Clear buffer
		buffer.fill(0);

		let center_x = (window_size.width / 2) as usize;
		let center_y = (window_size.height / 2) as usize;

		if delta.x == 0.0 && delta.y == 0.0 {
			// Just draw a colored dot in center
			let color = if self.grabbed { 0x00FF00 } else { 0xFF0000 };
			let dot_size = 5;
			for y in (center_y - dot_size)..(center_y + dot_size) {
				for x in (center_x - dot_size)..(center_x + dot_size) {
					if x < window_size.width as usize && y < window_size.height as usize {
						buffer[x + (y * window_size.width as usize)] = color;
					}
				}
			}
		} else {
			// Draw line in direction of movement
			let movement = vec2(delta.x as f32, delta.y as f32) * 20.0;

			// Draw line in steps
			let steps = 40;
			let thickness = 3; // How many pixels thick to make the line
			for t in 0..steps {
				let fraction = (t as f32) / (steps as f32);
				let point = movement * fraction;
				let center_point_x = (center_x as f32 + point.x) as isize;
				let center_point_y = (center_y as f32 + point.y) as isize;

				// Draw a square of pixels around each point
				for dy in -thickness..=thickness {
					for dx in -thickness..=thickness {
						let x = (center_point_x + dx) as usize;
						let y = (center_point_y + dy) as usize;
						if x < window_size.width as usize && y < window_size.height as usize {
							buffer[x + (y * window_size.width as usize)] = 0xFFFFFF;
						}
					}
				}
			}
		}

		buffer.present().unwrap();
	}

	fn handle_mouse_input(&mut self, state: ElementState, button: MouseButton) {
		if !self.grabbed {
			if state == ElementState::Released && button == MouseButton::Left {
				self.set_grab(true);
			}
			return;
		}
		let btn_id = match button {
			MouseButton::Left => input_event_codes::BTN_LEFT!(),
			MouseButton::Right => input_event_codes::BTN_RIGHT!(),
			MouseButton::Middle => input_event_codes::BTN_MIDDLE!(),
			MouseButton::Back => input_event_codes::BTN_BACK!(),
			MouseButton::Forward => input_event_codes::BTN_FORWARD!(),
			MouseButton::Other(n) => n,
		};
		send_input_ipc(Message::MouseButton {
			button: btn_id as u32,
			pressed: state == ElementState::Pressed,
		})
	}

	fn handle_keyboard_input(&mut self, input: KeyEvent) {
		if input.logical_key.as_ref() == Key::Character("q")
			&& input.state == ElementState::Released
			&& self.modifiers.state().super_key()
		{
			self.set_grab(false);
			return;
		}
		let pressed = input.state == ElementState::Pressed;

		let Some(keycode) = input.physical_key.to_scancode() else {
			return;
		};
		let keycode = keycode + 8;
		send_input_ipc(Message::Key { keycode, pressed });
	}

	const GRABBED_WINDOW_TITLE: &'static str = "Manifold Input (super+q to release cursor)";
	const UNGRABBED_WINDOW_TITLE: &'static str = "Manifold Input (click to grab input)";
	fn set_grab(&mut self, grab: bool) {
		if grab == self.grabbed {
			return;
		}
		self.grabbed = grab;

		// self.window.set_cursor_visible(!grab);

		let window_title = if grab {
			Self::GRABBED_WINDOW_TITLE
		} else {
			Self::UNGRABBED_WINDOW_TITLE
		};

		let grab = if grab {
			self.window
				.set_cursor_grab(CursorGrabMode::Locked)
				.or_else(|_| self.window.set_cursor_grab(CursorGrabMode::Confined))
		} else {
			self.window.set_cursor_grab(CursorGrabMode::None)
		};
		if grab.is_ok() {
			self.window.set_title(window_title);
		}
	}
}
