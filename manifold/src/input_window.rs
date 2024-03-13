use eclipse::StateChange;
use glam::{vec2, Vec2};
use ipc::{send_input_ipc, Message};
use map_range::MapRange;
use softbuffer::{Context, Surface};
use std::{
	num::NonZeroU32,
	process::exit,
	sync::mpsc::{self, Sender},
};
use winit::{
	dpi::{LogicalPosition, PhysicalPosition, Size},
	event::{
		ElementState, Event, KeyboardInput, ModifiersState, MouseButton, VirtualKeyCode,
		WindowEvent,
	},
	event_loop::EventLoop,
	window::{CursorGrabMode, Window, WindowBuilder},
};

fn line_dist(p: Vec2, l1: Vec2, l2: Vec2, thickness: f32) -> f32 {
	let pa = p - l1;
	let ba = l2 - l1;
	let t = pa.dot(ba) / ba.dot(ba);
	let h = t.clamp(0.0, 1.0);
	(pa - (ba * h)).length() - thickness
}

pub struct InputWindow {
	window: Window,
	surface: Surface,
	cursor_position: Option<LogicalPosition<u32>>,
	grabbed: bool,
	modifiers: ModifiersState,
	input_loop_tx: Sender<StateChange>,
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

		let (input_loop_tx, rx) = mpsc::channel();
		std::thread::spawn(move || eclipse::input_loop(false, rx));

		let surface = unsafe { Surface::new(&Context::new(&window).unwrap(), &window) }.unwrap();

		let mut input_window = InputWindow {
			window,
			surface,
			cursor_position: None,
			grabbed: true,
			modifiers: ModifiersState::empty(),
			input_loop_tx,
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
			WindowEvent::CursorMoved { position, .. } => self.handle_mouse_move(position),
			WindowEvent::KeyboardInput { input, .. } => self.handle_keyboard_input(input),
			WindowEvent::ModifiersChanged(state) => self.modifiers = state,

			WindowEvent::CursorEntered { .. } => {
				send_input_ipc(Message::ResetInput);
				self.input_loop_tx.send(StateChange::Enable).unwrap();
			}
			WindowEvent::CursorLeft { .. } => {
				self.input_loop_tx.send(StateChange::Disable).unwrap();
				send_input_ipc(Message::ResetInput);
			}

			WindowEvent::Destroyed => {
				self.input_loop_tx.send(StateChange::Stop).unwrap();
				send_input_ipc(Message::ResetInput);
				send_input_ipc(Message::Disconnect);
				exit(0);
			}
			WindowEvent::CloseRequested => {
				self.input_loop_tx.send(StateChange::Stop).unwrap();
				send_input_ipc(Message::ResetInput);
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

		let Some(mouse_position) = self.cursor_position else {
			return;
		};
		let window_center = vec2(
			window_size.width as f32 / 2.0,
			window_size.height as f32 / 2.0,
		);
		let mouse_position = vec2(mouse_position.x as f32, mouse_position.y as f32);
		let delta = mouse_position - window_center;
		let l1 = window_center;
		let l2 = window_center + (delta * 4.0);
		let thickness = 10.0;

		for x in 0..window_size.width {
			for y in 0..window_size.height {
				let dist = line_dist(vec2(x as f32, y as f32), l1, l2, thickness);
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
			let center_position = LogicalPosition::new(
				window_size.width as f64 / 2.0,
				window_size.height as f64 / 2.0,
			);
			self.window.set_cursor_position(center_position).unwrap();
		}
	}

	fn handle_mouse_input(&mut self, state: ElementState, button: MouseButton) {
		if !self.grabbed {
			if state == ElementState::Released && button == MouseButton::Left {
				self.set_grab(true);
			}
		}
	}

	fn handle_keyboard_input(&mut self, input: KeyboardInput) {
		if input.virtual_keycode == Some(VirtualKeyCode::Z)
			&& input.state == ElementState::Released
			&& self.modifiers.logo()
		{
			self.set_grab(false);
		}
	}

	const GRABBED_WINDOW_TITLE: &'static str = "Manifold Input (super+z to release cursor)";
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
