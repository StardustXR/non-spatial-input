use eclipse::StateChange;
use glam::{vec2, Vec2};
use ipc::{send_input_ipc, Message};
use map_range::MapRange;
use softbuffer::{Context, Surface};
use std::{
	iter::repeat_with,
	num::NonZeroU32,
	process::exit,
	rc::Rc,
	sync::mpsc::{self, Sender},
};
use winit::{
	dpi::{LogicalPosition, PhysicalPosition, Size},
	event::{
		DeviceEvent, ElementState, Event, KeyEvent, Modifiers, MouseButton, MouseScrollDelta,
		WindowEvent,
	},
	event_loop::{EventLoop, EventLoopWindowTarget},
	keyboard::{Key, KeyCode, ModifiersKeyState, NativeKey, PhysicalKey},
	platform::scancode::PhysicalKeyExtScancode,
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
	window: Rc<Window>,
	surface: Surface<Rc<Window>, Rc<Window>>,
	mouse_delta: Option<LogicalPosition<f64>>,
	grabbed: bool,
	modifiers: Modifiers,
	input_loop_tx: Sender<StateChange>,
}
impl InputWindow {
	pub fn new(event_loop: &EventLoop<()>) -> Self {
		let size = Size::Logical([128, 128].into());
		let window = Rc::new(
			WindowBuilder::new()
				.with_title("Manifold")
				.with_min_inner_size(size)
				.build(&event_loop)
				.unwrap(),
		);

		let (input_loop_tx, rx) = mpsc::channel();
		std::thread::spawn(move || eclipse::input_loop(false, rx));

		let context = softbuffer::Context::new(window.clone()).unwrap();
		let surface = softbuffer::Surface::new(&context, window.clone()).unwrap();

		let mut input_window = InputWindow {
			window,
			surface,
			mouse_delta: None,
			grabbed: true,
			modifiers: Modifiers::default(),
			input_loop_tx,
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
			// Doesn't seem to do anything, at least on wayland
			// Event::DeviceEvent {
			// 	event: DeviceEvent::MouseWheel { delta },
			// 	..
			// } => {
			// 	dbg!(delta);
			// 	match delta {
			// 		MouseScrollDelta::LineDelta(x, y) => {
			// 			send_input_ipc(Message::MouseAxisContinuous(vec2(x, y).into()))
			// 		}
			// 		MouseScrollDelta::PixelDelta(p) => send_input_ipc(Message::MouseAxisDiscrete(
			// 			vec2(p.x as f32, p.y as f32).into(),
			// 		)),
			// 	}
			// }
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

		let delta = vec2(delta.x as f32, delta.y as f32);
		let window_center = vec2(
			window_size.width as f32 / 2.0,
			window_size.height as f32 / 2.0,
		);

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

	fn handle_mouse_input(&mut self, state: ElementState, button: MouseButton) {
		if !self.grabbed {
			if state == ElementState::Released && button == MouseButton::Left {
				self.set_grab(true);
			}
			return;
		}
		let btn_id = match button {
			MouseButton::Left => 272,
			MouseButton::Right => 273,
			MouseButton::Middle => todo!(),
			MouseButton::Back => todo!(),
			MouseButton::Forward => todo!(),
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
			dbg!(":(");
			return;
		};
		send_input_ipc(Message::Key { keycode, pressed });
	}

	const GRABBED_WINDOW_TITLE: &'static str = "Manifold Input (super+q to release cursor)";
	const UNGRABBED_WINDOW_TITLE: &'static str = "Manifold Input (click to grab input)";
	fn set_grab(&mut self, grab: bool) {
		if grab == self.grabbed {
			return;
		}
		self.grabbed = grab;

		self.window.set_cursor_visible(!grab);

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
