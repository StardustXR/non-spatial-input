pub mod handlers;

use color_eyre::eyre::Result;
use glam::Quat;
use handlers::{PointerHandler, PulseReceiverCollector};
use input_event_codes::{BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
use ipc::receive_input_async_ipc;
use parking_lot::Mutex;
use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};
use stardust_xr_fusion::{
	client::Client,
	core::values::{color::rgba_linear, Datamap, Vector2},
	data::{PulseReceiver, PulseSender, PulseSenderAspect},
	drawable::Lines,
	fields::{FieldRefAspect, RayMarchResult},
	input::{InputDataType, InputMethod, InputMethodAspect, Pointer},
	node::NodeType,
	objects::hmd,
	root::{ClientState, FrameInfo, RootAspect, RootHandler},
	spatial::{SpatialAspect, SpatialRef, Transform},
	HandlerWrapper,
};
use stardust_xr_molecules::{
	keyboard::{KeyboardEvent, KEYBOARD_MASK},
	lines::{circle, LineExt},
};
use std::{io::IsTerminal, sync::Arc, time::Duration};
use tokio::{sync::watch, task::JoinSet};
use tracing::{info, info_span};

// degrees per pixel, constant for now since i'm lazy
const MOUSE_SENSITIVITY: f32 = 0.1;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PointerDatamap {
	mouse: (),
	select: f32,
	middle: f32,
	context: f32,
	grab: f32,
	scroll_continuous: Vector2<f32>,
	scroll_discrete: Vector2<f32>,
	raw_input_events: FxHashSet<u32>,
}
impl Default for PointerDatamap {
	fn default() -> Self {
		Self {
			mouse: (),
			select: 0.0,
			middle: 0.0,
			context: 0.0,
			grab: 0.0,
			scroll_continuous: [0.0; 2].into(),
			scroll_discrete: [0.0; 2].into(),
			raw_input_events: FxHashSet::default(),
		}
	}
}

#[tokio::main]
async fn main() -> Result<()> {
	if std::io::stdin().is_terminal() {
		panic!("You need to pipe azimuth or eclipse's output into this e.g. `eclipse | azimuth`");
	}
	// console_subscriber::init();
	color_eyre::install().unwrap();
	let (client, event_loop) = Client::connect_with_async_loop()
		.await
		.expect("Couldn't connect");
	let hmd = hmd(&client).await.unwrap();

	// Pointer stuff
	let pointer = InputMethod::create(
		client.get_root(),
		Transform::identity(),
		InputDataType::Pointer(Pointer {
			origin: [0.0; 3].into(),
			orientation: Quat::IDENTITY.into(),
			deepest_point: [0.0; 3].into(),
		}),
		&Datamap::from_typed(PointerDatamap::default())?,
	)?;
	let handler = PointerHandler::new(pointer.alias());
	let pointer = pointer.wrap(handler)?;
	let _ = pointer
		.node()
		.set_relative_transform(&hmd, Transform::from_translation([0.0; 3]));

	let line = circle(8, 0.0, 0.001)
		.thickness(0.0025)
		.color(rgba_linear!(1.0, 1.0, 1.0, 1.0));
	let pointer_reticle = Lines::create(
		pointer.node().as_ref(),
		Transform::from_translation([0.0, 0.0, -0.5]),
		&[line],
	)?;

	// Keyboard stuff
	let keyboard_sender = PulseSender::create(
		pointer.node().as_ref(),
		Transform::identity(),
		&KEYBOARD_MASK,
	)?
	.wrap(PulseReceiverCollector::default())?;
	let (hovered_keyboard_tx, hovered_keyboard) = watch::channel::<Option<PulseReceiver>>(None);
	let (frame_count_tx, frame_count_rx) = watch::channel(0);

	// doing the actual handling
	let input_loop = tokio::task::spawn(input_loop(
		client.clone(),
		pointer.node().alias(),
		keyboard_sender.node().alias(),
		hovered_keyboard,
		frame_count_rx,
	));
	tokio::spawn(reconnect_keyboard_loop(
		pointer.node().alias(),
		keyboard_sender.wrapped().clone(),
		hovered_keyboard_tx,
	));
	let _client_root = client.get_root().alias().wrap(Root {
		root: client.get_root().alias(),
		hmd,
		pointer,
		pointer_reticle,
		frame_count_tx,
	})?;

	tokio::select! {
		biased;
		_ = tokio::signal::ctrl_c() => Ok(()),
		e = event_loop => e?.map_err(|e| e.into()),
		_ = input_loop => Ok(()),
	}
}

async fn input_loop(
	client: Arc<Client>,
	pointer: InputMethod,
	keyboard_sender: PulseSender,
	hovered_keyboard: watch::Receiver<Option<PulseReceiver>>,
	frame_count_rx: watch::Receiver<u32>,
) {
	let mut keymap_id: Option<u64> = None;

	let mut yaw = 0.0;
	let mut pitch = 0.0;

	let mut mouse_buttons = FxHashSet::default();
	let mut pointer_datamap = PointerDatamap::default();
	let mut old_frame_count = 0_u32;
	// let mut past_time = Instant::now();

	while let Ok(message) = receive_input_async_ipc().await {
		let span = info_span!("handle ipc message");
		let _span_enter = span.enter();
		if *frame_count_rx.borrow() > old_frame_count {
			old_frame_count = *frame_count_rx.borrow();
			pointer_datamap.scroll_continuous = [0.0; 2].into();
			pointer_datamap.scroll_discrete = [0.0; 2].into();
		}
		// println!(
		// 	"time since last event: {}",
		// 	past_time.elapsed().as_secs_f32()
		// );
		// past_time = Instant::now();
		match message {
			ipc::Message::Keymap(keymap) => {
				info!("IPC keymap message");
				let Ok(future) = client.register_xkb_keymap(keymap) else {
					continue;
				};
				let Ok(new_keymap_id) = future.await else {
					continue;
				};
				keymap_id.replace(new_keymap_id);
			}
			ipc::Message::Key { keycode, pressed } => {
				info!("IPC key message");
				let Some(hovered_keyboard) = &*hovered_keyboard.borrow() else {
					continue;
				};
				let Some(keymap_id) = keymap_id else {
					continue;
				};
				KeyboardEvent {
					keyboard: (),
					xkbv1: (),
					keymap_id,
					keys: vec![if pressed {
						keycode as i32
					} else {
						-(keycode as i32)
					}]
					.into_iter()
					.collect(),
				}
				.send_event(&keyboard_sender, &[hovered_keyboard])
			}
			ipc::Message::MouseMove(delta) => {
				info!("IPC mouse move message");
				yaw += delta.x * MOUSE_SENSITIVITY;
				pitch += delta.y * MOUSE_SENSITIVITY;
				pitch = pitch.clamp(-90.0, 90.0);

				let rotation_x = Quat::from_rotation_x(-pitch.to_radians());
				let rotation_y = Quat::from_rotation_y(-yaw.to_radians());
				let _ =
					pointer.set_local_transform(Transform::from_rotation(rotation_y * rotation_x));
			}
			ipc::Message::MouseButton { button, pressed } => {
				info!("IPC mouse button message");
				if button > 255 {
					if pressed {
						mouse_buttons.insert(button);
					} else {
						mouse_buttons.remove(&button);
					}
				}
				pointer_datamap.raw_input_events.clone_from(&mouse_buttons);
				match button {
					BTN_LEFT!() => {
						pointer_datamap.select = if pressed { 1.0 } else { 0.0 };
					}
					BTN_MIDDLE!() => {
						pointer_datamap.middle = if pressed { 1.0 } else { 0.0 };
					}
					BTN_RIGHT!() => {
						pointer_datamap.context = if pressed { 1.0 } else { 0.0 };
					}
					_ => {
						// idk why this number but that's what it spits out for side mousebuttons lol
						pointer_datamap.grab = if pressed { 1.0 } else { 0.0 };
					} // b => {
					  // 	println!("Unknown mouse button {b}");
					  // 	continue;
					  // }
				}
				pointer
					.set_datamap(&Datamap::from_typed(pointer_datamap.clone()).unwrap())
					.unwrap();
			}
			ipc::Message::MouseAxisContinuous(scroll) => {
				info!("IPC mouse axis continuous message");
				let scroll_continuous = &mut pointer_datamap.scroll_continuous;
				*scroll_continuous = [
					scroll_continuous.x + scroll.x,
					scroll_continuous.y + scroll.y,
				]
				.into();
				pointer
					.set_datamap(&Datamap::from_typed(pointer_datamap.clone()).unwrap())
					.unwrap();
			}
			ipc::Message::MouseAxisDiscrete(scroll) => {
				info!("IPC mouse axis discrete message");
				let scroll_discrete = &mut pointer_datamap.scroll_discrete;
				*scroll_discrete =
					[scroll_discrete.x + scroll.x, scroll_discrete.y + scroll.y].into();
				pointer
					.set_datamap(&Datamap::from_typed(pointer_datamap.clone()).unwrap())
					.unwrap();
			}
			ipc::Message::ResetInput => (),
			ipc::Message::Disconnect => break,
		}
	}
}

async fn reconnect_keyboard_loop(
	pointer: InputMethod,
	keyboard_sender: Arc<Mutex<PulseReceiverCollector>>,
	hovered_keyboard_tx: watch::Sender<Option<PulseReceiver>>,
) {
	loop {
		let mut closest_hit: Option<(PulseReceiver, RayMarchResult)> = None;
		let mut join = JoinSet::new();
		for (receiver, field) in keyboard_sender.lock().0.values() {
			let field = field.alias();
			let pointer = pointer.alias();
			let receiver = receiver.alias();
			join.spawn(async move {
				(
					receiver,
					field.ray_march(&pointer, [0.0; 3], [0.0, 0.0, -1.0]).await,
				)
			});
		}
		while let Some(res) = join.join_next().await {
			let Ok((receiver, Ok(ray_info))) = res else {
				continue;
			};
			if ray_info.min_distance > 0.0 || ray_info.deepest_point_distance <= 0.001 {
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
		tokio::time::sleep(Duration::from_secs_f64(0.1)).await
	}
}

struct Root {
	root: stardust_xr_fusion::root::Root,
	hmd: SpatialRef,
	pointer: HandlerWrapper<InputMethod, PointerHandler>,
	pointer_reticle: Lines,
	frame_count_tx: watch::Sender<u32>,
}
impl RootHandler for Root {
	fn frame(&mut self, _info: FrameInfo) {
		self.frame_count_tx.send_modify(|i| *i += 1);
		let _ = self
			.pointer
			.node()
			.set_relative_transform(&self.hmd, Transform::from_translation([0.0; 3]));
		self.pointer
			.wrapped()
			.lock()
			.update_pointer(self.pointer_reticle.alias());
	}
	fn save_state(&mut self) -> Result<ClientState> {
		ClientState::from_root(&self.root)
	}
}
