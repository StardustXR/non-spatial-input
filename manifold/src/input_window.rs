use std::{num::NonZeroU32, process::exit};

use as_raw_xcb_connection::{xcb_connection_t, ValidConnection};
use glam::vec2;
use ipc::{send_input_ipc, Message};
use map_range::MapRange;
use mint::Vector2;
use sdfu::{Line, SDF};
use softbuffer::{Context, Surface};
use winit::{
	dpi::{LogicalPosition, PhysicalPosition, Size},
	event::{
		ElementState, Event, KeyboardInput, ModifiersState, MouseButton, MouseScrollDelta,
		VirtualKeyCode, WindowEvent,
	},
	event_loop::EventLoop,
	platform::x11::WindowExtX11,
	window::{CursorGrabMode, Window, WindowBuilder},
};
use xkbcommon::xkb::{
	self,
	ffi::XKB_KEYMAP_FORMAT_TEXT_V1,
	x11::{get_core_keyboard_device_id, keymap_new_from_device},
	Keymap, KEYMAP_COMPILE_NO_FLAGS,
};

pub struct InputWindow {
	window: Window,
	surface: Surface,
	cursor_position: Option<LogicalPosition<u32>>,
	grabbed: bool,
	modifiers: ModifiersState,
}
impl InputWindow {
	pub fn new(event_loop: &EventLoop<()>) -> Self {
		let size = Size::Logical([512, 512].into());
		let window = WindowBuilder::new()
			.with_title("Manifold")
			.with_min_inner_size(size)
			.with_max_inner_size(size)
			.with_inner_size(size)
			.with_resizable(false)
			.build(event_loop)
			.unwrap();

		let keymap = match window.xcb_connection() {
			Some(raw_conn) => {
				let connection = unsafe { ValidConnection::new(raw_conn as *mut xcb_connection_t) };
				keymap_new_from_device(
					&xkb::Context::new(0),
					&connection,
					get_core_keyboard_device_id(&connection),
					KEYMAP_COMPILE_NO_FLAGS,
				)
			}
			None => Keymap::new_from_names(&xkb::Context::new(0), "", "", "", "", None, 0).unwrap(),
		};
		send_input_ipc(Message::Keymap(
			keymap.get_as_string(XKB_KEYMAP_FORMAT_TEXT_V1),
		));

		let surface = unsafe { Surface::new(&Context::new(&window).unwrap(), &window) }.unwrap();

		let mut input_window = InputWindow {
			window,
			surface,
			cursor_position: None,
			grabbed: true,
			modifiers: ModifiersState::empty(),
		};
		input_window.set_grab(false);
		input_window
	}

	pub fn handle_event(&mut self, event: Event<()>) {
		match event {
			Event::WindowEvent { event, .. } => self.handle_window_event(event),
			Event::RedrawRequested(_) => self.redraw(),
			_ => (),
		}
	}

	fn handle_window_event(&mut self, event: WindowEvent) {
		match event {
			WindowEvent::MouseInput { state, button, .. } => self.handle_mouse_input(state, button),
			WindowEvent::MouseWheel { delta, .. } => self.handle_axis(delta),
			WindowEvent::CursorMoved { position, .. } => self.handle_mouse_move(position),
			WindowEvent::KeyboardInput { input, .. } => self.handle_keyboard_input(input),
			WindowEvent::ModifiersChanged(state) => self.modifiers = state,

			WindowEvent::Destroyed => {
				send_input_ipc(Message::Disconnect);
				exit(0);
			}
			WindowEvent::CloseRequested => {
				send_input_ipc(Message::Disconnect);
				exit(0);
			}
			_ => (),
		}
	}

	fn redraw(&mut self) {
		let window_size = self.window.inner_size();
		self.surface
			.resize(
				NonZeroU32::new(window_size.width).unwrap(),
				NonZeroU32::new(window_size.height).unwrap(),
			)
			.unwrap();
		let mut buffer = self.surface.buffer_mut().unwrap();

		let Some(mouse_position) = self.cursor_position else {return};
		let window_center = vec2(
			window_size.width as f32 / 2.0,
			window_size.height as f32 / 2.0,
		);
		let mouse_position = vec2(mouse_position.x as f32, mouse_position.y as f32);
		let delta = mouse_position - window_center;
		let motion_vector = Line::new(window_center, window_center + (delta * 4.0), 10.0);

		for x in 0..window_size.width {
			for y in 0..window_size.height {
				let dist = motion_vector.dist(vec2(x as f32, y as f32));
				let intensity = dist.map_range(0.5..-0.5, 0.0..1.0).clamp(0.0, 1.0);
				let intensity_u8 = (intensity * 255.0) as u32;
				// let intensity_u8 = 255;
				buffer[(x + (y * window_size.width)) as usize] =
					intensity_u8 | (intensity_u8 << 8) | (intensity_u8 << 16);
			}
		}
		buffer.present().unwrap();
	}

	fn handle_mouse_move(&mut self, position: PhysicalPosition<f64>) {
		self.cursor_position = if self.grabbed {
			Some(position.to_logical::<u32>(self.window.scale_factor()))
		} else {
			None
		};

		if self.grabbed {
			self.window.request_redraw();
			let window_size = self.window.inner_size();
			let cursor_position = position.to_logical::<f64>(self.window.scale_factor());
			let center_position = LogicalPosition::new(
				window_size.width as f64 / 2.0,
				window_size.height as f64 / 2.0,
			);
			let cursor_delta = Vector2::from_slice(&[
				(cursor_position.x - center_position.x) as f32,
				(cursor_position.y - center_position.y) as f32,
			]);
			send_input_ipc(Message::MouseMove(cursor_delta));

			self.window.set_cursor_position(center_position).unwrap();
		}
	}

	fn handle_mouse_input(&mut self, state: ElementState, button: MouseButton) {
		if !self.grabbed {
			if state == ElementState::Released && button == MouseButton::Left {
				self.set_grab(true);
			}
		} else {
			let button = match button {
				MouseButton::Left => input_event_codes::BTN_LEFT!(),
				MouseButton::Right => input_event_codes::BTN_RIGHT!(),
				MouseButton::Middle => input_event_codes::BTN_MIDDLE!(),
				MouseButton::Other(_) => {
					return;
				}
			};
			send_input_ipc(Message::MouseButton {
				button,
				pressed: state == ElementState::Pressed,
			});
		}
	}

	fn handle_axis(&mut self, delta: MouseScrollDelta) {
		if self.grabbed {
			send_input_ipc(match delta {
				MouseScrollDelta::LineDelta(right, down) => {
					Message::MouseAxisDiscrete([right, down].into())
				}
				MouseScrollDelta::PixelDelta(offset) => {
					Message::MouseAxisContinuous([-offset.x as f32, -offset.y as f32].into())
				}
			});
		}
	}

	fn handle_keyboard_input(&mut self, input: KeyboardInput) {
		if input.virtual_keycode == Some(VirtualKeyCode::Escape)
			&& input.state == ElementState::Released
			&& self.modifiers.ctrl()
		{
			self.set_grab(false);
		} else {
			send_input_ipc(Message::Key {
				keycode: input.scancode,
				pressed: input.state == ElementState::Pressed,
			});
		}
	}

	const GRABBED_WINDOW_TITLE: &'static str = "Manifold Input (ctrl+esc to release cursor)";
	const UNGRABBED_WINDOW_TITLE: &'static str = "Manifold Input (click to grab input)";
	fn set_grab(&mut self, grab: bool) {
		if grab == self.grabbed {
			return;
		}
		self.grabbed = grab;

		self.window.set_cursor_visible(!grab);
		if grab {
			let window_size = self.window.inner_size();
			let center_position =
				LogicalPosition::new(window_size.width / 2, window_size.height / 2);
			self.window.set_cursor_position(center_position).unwrap();
		}
		let window_title = if grab {
			Self::GRABBED_WINDOW_TITLE
		} else {
			Self::UNGRABBED_WINDOW_TITLE
		};

		let grab = if grab {
			CursorGrabMode::Confined
		} else {
			CursorGrabMode::None
		};

		if self.window.set_cursor_grab(grab).is_ok() {
			self.window.set_title(window_title);
		}
	}
}
