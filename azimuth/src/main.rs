use color::rgba;
use color_eyre::Result;
use glam::{Quat, Vec3};
use input_event_codes::{BTN_LEFT, BTN_RIGHT};
use ipc::receive_input_async_ipc;
use mint::Vector2;
use serde::{Deserialize, Serialize};
use stardust_xr_fusion::{
	client::{Client, FrameInfo, RootHandler},
	core::values::Transform,
	data::{PulseReceiver, PulseSender},
	drawable::Lines,
	fields::{Field, RayMarchResult},
	input::{InputHandler, InputMethod, PointerInputMethod},
	node::NodeType,
};
use stardust_xr_molecules::{
	datamap::Datamap,
	keyboard::{KeyboardEvent, KEYBOARD_MASK},
	lines::{circle, make_line_points},
	DummyHandler,
};
use std::{io::IsTerminal, sync::Arc};
use tokio::{
	sync::{watch, Notify},
	task::JoinSet,
};

// degrees per pixel, constant for now since i'm lazy
const MOUSE_SENSITIVITY: f32 = 0.05;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PointerDatamap {
	select: f32,
	grab: f32,
	scroll: Vector2<f32>,
}
impl Default for PointerDatamap {
	fn default() -> Self {
		Self {
			select: 0.0,
			grab: 0.0,
			scroll: [0.0; 2].into(),
		}
	}
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
	if std::io::stdin().is_terminal() {
		panic!("You need to pipe azimuth or eclipse's output into this e.g. `eclipse | azimuth`");
	}
	color_eyre::install().unwrap();
	let (client, event_loop) = Client::connect_with_async_loop()
		.await
		.expect("Couldn't connect");

	// Pointer stuff
	let pointer = PointerInputMethod::create(client.get_root(), Transform::identity(), None)?;
	let _ = pointer.set_position(Some(client.get_hmd()), [0.0; 3]);
	let line_points = make_line_points(&circle(8, 0.0, 0.001), 0.0025, rgba!(1.0, 1.0, 1.0, 1.0));
	let pointer_reticle = Lines::create(
		&pointer,
		Transform::from_position([0.0, 0.0, -0.5]),
		&line_points,
		true,
	)?;

	// Keyboard stuff
	let keyboard_sender =
		PulseSender::create(&pointer, Transform::identity(), &KEYBOARD_MASK)?.wrap(DummyHandler)?;
	let (hovered_keyboard_tx, hovered_keyboard) = watch::channel::<Option<PulseReceiver>>(None);

	let frame_notifier = Arc::new(Notify::new());
	let _client_root = client.wrap_root(FrameNotifier(frame_notifier.clone()))?;

	let input_loop = tokio::task::spawn(input_loop(
		client.clone(),
		keyboard_sender.node().alias(),
		hovered_keyboard,
		pointer.alias(),
	));
	let pointer_frame_loop = tokio::task::spawn(pointer_frame_loop(
		frame_notifier.clone(),
		pointer.alias(),
		pointer_reticle,
	));
	let keyboard_frame_loop = tokio::task::spawn(keyboard_frame_loop(
		frame_notifier.clone(),
		pointer.alias(),
		keyboard_sender.node().alias(),
		hovered_keyboard_tx,
	));

	let result = tokio::select! {
		biased;
		_ = tokio::signal::ctrl_c() => Ok(()),
		e = event_loop => e?.map_err(|e| e.into()),
		_ = input_loop => Ok(()),
	};

	pointer_frame_loop.abort();
	keyboard_frame_loop.abort();

	result
}

async fn input_loop(
	client: Arc<Client>,
	keyboard_sender: PulseSender,
	hovered_keyboard: watch::Receiver<Option<PulseReceiver>>,
	pointer: InputMethod,
) {
	let mut keymap_id: Option<String> = None;

	let mut yaw = 0.0;
	let mut pitch = 0.0;
	let mut pointer_datamap = Datamap::create(PointerDatamap::default());

	while let Ok(message) = receive_input_async_ipc().await {
		match message {
			ipc::Message::Keymap(keymap) => {
				let Ok(register_keymap_future) = client.register_keymap(&keymap) else {continue};
				let Ok(new_keymap_id) = register_keymap_future.await else {continue};
				keymap_id.replace(new_keymap_id);
			}
			ipc::Message::Key { keycode, pressed } => {
				let Some(hovered_keyboard) = &*hovered_keyboard.borrow() else {continue};
				let Some(keymap_id) = keymap_id.clone() else {continue};
				KeyboardEvent {
					keyboard: (),
					xkbv1: (),
					keymap_id,
					keys: vec![if pressed {
						keycode as i32
					} else {
						-(keycode as i32)
					}],
				}
				.send_event(&keyboard_sender, &[hovered_keyboard])
			}
			ipc::Message::MouseMove(delta) => {
				yaw += delta.x * MOUSE_SENSITIVITY;
				pitch += delta.y * MOUSE_SENSITIVITY;
				pitch = pitch.clamp(-90.0, 90.0);

				let rotation_x = Quat::from_rotation_x(-pitch.to_radians());
				let rotation_y = Quat::from_rotation_y(-yaw.to_radians());
				let _ = pointer.set_rotation(None, rotation_y * rotation_x);
			}
			ipc::Message::MouseButton { button, pressed } => {
				match button {
					BTN_LEFT!() => pointer_datamap.data().select = if pressed { 1.0 } else { 0.0 },
					BTN_RIGHT!() => pointer_datamap.data().grab = if pressed { 1.0 } else { 0.0 },
					b => {
						println!("Unknown mouse button {b}");
						continue;
					}
				}
				pointer_datamap.update_input_method(&pointer).unwrap();
			}
			ipc::Message::MouseAxisContinuous(_) => todo!(),
			ipc::Message::MouseAxisDiscrete(_) => todo!(),
			ipc::Message::Disconnect => break,
		}
	}
}

async fn pointer_frame_loop(
	frame_notifier: Arc<Notify>,
	pointer: InputMethod,
	pointer_reticle: Lines,
) {
	loop {
		frame_notifier.notified().await;
		let mut closest_hits: Option<(Vec<InputHandler>, RayMarchResult)> = None;
		let mut join = JoinSet::new();
		for handler in pointer.input_handlers().values() {
			let Some(field) = handler.field() else {continue};
			let Ok(ray_march_result) = field.ray_march(&pointer, [0.0; 3], [0.0, 0.0, -1.0]) else {continue};
			let handler = handler.alias();
			join.spawn(async move { (handler, ray_march_result.await) });
		}

		while let Some(res) = join.join_next().await {
			let Ok((handler, Ok(ray_info))) = res else {continue};
			if !ray_info.hit() {
				continue;
			}
			if let Some((hit_handlers, hit_info)) = &mut closest_hits {
				if ray_info.deepest_point_distance == hit_info.deepest_point_distance {
					hit_handlers.push(handler);
				} else if ray_info.deepest_point_distance < hit_info.deepest_point_distance {
					*hit_handlers = vec![handler];
					*hit_info = ray_info;
				}
			} else {
				closest_hits.replace((vec![handler], ray_info));
			}
		}

		if let Some((hit_handlers, hit_info)) = closest_hits {
			let _ = pointer.set_handler_order(hit_handlers.iter().collect::<Vec<_>>().as_slice());
			let _ = pointer_reticle
				.set_position(Some(&pointer), Vec3::from(hit_info.deepest_point()) * 0.95);
		} else {
			let _ = pointer.set_handler_order(&[]);
			let _ = pointer_reticle.set_position(Some(&pointer), [0.0, 0.0, -0.5]);
		}
	}
}

async fn keyboard_frame_loop(
	frame_notifier: Arc<Notify>,
	pointer: InputMethod,
	keyboard_sender: PulseSender,
	hovered_keyboard_tx: watch::Sender<Option<PulseReceiver>>,
) {
	loop {
		frame_notifier.notified().await;
		let mut closest_hit: Option<(PulseReceiver, RayMarchResult)> = None;
		let mut join = JoinSet::new();
		for (receiver, field) in keyboard_sender.receivers().values() {
			let Ok(ray_march_result) = field.ray_march(&pointer, [0.0; 3], [0.0, 0.0, -1.0]) else {continue};
			let receiver = receiver.alias();
			join.spawn(async move { (receiver, ray_march_result.await) });
		}

		while let Some(res) = join.join_next().await {
			let Ok((receiver, Ok(ray_info))) = res else {continue};
			if !ray_info.hit() || ray_info.deepest_point_distance <= 0.001 {
				continue;
			}
			if let Some((hit_receiver, hit_info)) = &mut closest_hit {
				if ray_info.deepest_point_distance < hit_info.deepest_point_distance {
					*hit_receiver = receiver;
					*hit_info = ray_info;
				}
			} else {
				closest_hit.replace((receiver, ray_info));
			}
		}
		let _ = hovered_keyboard_tx.send(closest_hit.map(|(r, _)| r));
	}
}

struct FrameNotifier(Arc<Notify>);
impl RootHandler for FrameNotifier {
	fn frame(&mut self, _info: FrameInfo) {
		self.0.notify_one()
	}
}
