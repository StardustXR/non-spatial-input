pub mod handlers;

use color::rgba_linear;
use color_eyre::Result;
use glam::{Quat, Vec3};
use handlers::{InputHandlerCollector, PulseReceiverCollector};
use input_event_codes::BTN_LEFT;
use ipc::receive_input_async_ipc;
use mint::Vector2;
use parking_lot::Mutex;
use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};
use stardust_xr_fusion::{
	client::{Client, ClientState, FrameInfo, RootHandler},
	core::values::Datamap,
	data::{PulseReceiver, PulseSender, PulseSenderAspect},
	drawable::{Line, Lines},
	fields::{FieldAspect, RayMarchResult},
	input::{InputHandler, InputMethodAspect, PointerInputMethod},
	node::NodeType,
	spatial::{SpatialAspect, Transform},
	HandlerWrapper,
};
use stardust_xr_molecules::{
	keyboard::{KeyboardEvent, KEYBOARD_MASK},
	lines::{circle, make_line_points},
};
use std::{io::IsTerminal, sync::Arc};
use tokio::{sync::watch, task::JoinSet};
use tracing::{info, info_span, instrument};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

// degrees per pixel, constant for now since i'm lazy
const MOUSE_SENSITIVITY: f32 = 0.01;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PointerDatamap {
	mouse: (),
	select: f32,
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
	tracing_subscriber::registry()
		.with(tracing_tracy::TracyLayer::new())
		.init();
	let (client, event_loop) = Client::connect_with_async_loop()
		.await
		.expect("Couldn't connect");

	// Pointer stuff
	let pointer = PointerInputMethod::create(
		client.get_root(),
		Transform::identity(),
		Datamap::from_typed(PointerDatamap::default())?,
	)?
	.wrap(InputHandlerCollector::default())?;
	let _ = pointer
		.node()
		.set_relative_transform(client.get_hmd(), Transform::from_translation([0.0; 3]));
	let line_points = make_line_points(
		circle(8, 0.0, 0.001),
		0.0025,
		rgba_linear!(1.0, 1.0, 1.0, 1.0),
	);
	let pointer_reticle = Lines::create(
		pointer.node().as_ref(),
		Transform::from_translation([0.0, 0.0, -0.5]),
		&[Line {
			points: line_points,
			cyclic: true,
		}],
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
	let _client_root = client.wrap_root(Root {
		pointer,
		pointer_reticle,
		keyboard_sender,
		hovered_keyboard_tx: Arc::new(hovered_keyboard_tx),
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
	pointer: PointerInputMethod,
	keyboard_sender: PulseSender,
	hovered_keyboard: watch::Receiver<Option<PulseReceiver>>,
	frame_count_rx: watch::Receiver<u32>,
) {
	let mut keymap_id: Option<String> = None;

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
				let Some(keymap_id) = keymap_id.clone() else {
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
				pointer_datamap.raw_input_events = mouse_buttons.clone();
				match button {
					BTN_LEFT!() => {
						pointer_datamap.select = if pressed { 1.0 } else { 0.0 };
					}
					8..=9 => {
						// idk why this number but that's what it spits out for side mousebuttons lol
						pointer_datamap.grab = if pressed { 1.0 } else { 0.0 };
						dbg!("holding right mouse button");
					}
					b => {
						println!("Unknown mouse button {b}");
						continue;
					}
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
			ipc::Message::Disconnect => break,
		}
	}
}

#[instrument]
fn update_pointer(
	pointer: PointerInputMethod,
	input_handler_collector: Arc<Mutex<InputHandlerCollector>>,
	pointer_reticle: Lines,
) {
	let mut closest_hits: Option<(Vec<InputHandler>, RayMarchResult)> = None;
	let mut join = JoinSet::new();
	for handler in input_handler_collector.lock().0.values() {
		let Some(field) = handler.field() else {
			continue;
		};
		let handler = handler.alias();
		let field = field.alias();
		let pointer = pointer.alias();
		join.spawn(async move {
			(
				handler,
				field.ray_march(&pointer, [0.0; 3], [0.0, 0.0, -1.0]).await,
			)
		});
	}
	tokio::spawn(async move {
		while let Some(res) = join.join_next().await {
			let Ok((handler, Ok(ray_info))) = res else {
				continue;
			};
			if ray_info.min_distance > 0.0 {
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
			let _ = pointer_reticle.set_relative_transform(
				&pointer,
				Transform::from_translation(
					Vec3::from(hit_info.ray_origin)
						+ Vec3::from(hit_info.ray_direction)
							* hit_info.deepest_point_distance
							* 0.95,
				),
			);
		} else {
			let _ = pointer.set_handler_order(&[]);
			let _ = pointer_reticle
				.set_relative_transform(&pointer, Transform::from_translation([0.0, 0.0, -0.5]));
		}
	});
}

#[instrument]
fn reconnect_keyboard(
	pointer: PointerInputMethod,
	keyboard_sender: Arc<Mutex<PulseReceiverCollector>>,
	hovered_keyboard_tx: Arc<watch::Sender<Option<PulseReceiver>>>,
) {
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
	tokio::task::spawn(async move {
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
	});
}

struct Root {
	pointer: HandlerWrapper<PointerInputMethod, InputHandlerCollector>,
	pointer_reticle: Lines,
	keyboard_sender: HandlerWrapper<PulseSender, PulseReceiverCollector>,
	hovered_keyboard_tx: Arc<watch::Sender<Option<PulseReceiver>>>,
	frame_count_tx: watch::Sender<u32>,
}
impl RootHandler for Root {
	fn frame(&mut self, _info: FrameInfo) {
		self.frame_count_tx.send_modify(|i| *i += 1);
		update_pointer(
			self.pointer.node().alias(),
			self.pointer.wrapped().clone(),
			self.pointer_reticle.alias(),
		);
		reconnect_keyboard(
			self.pointer.node().alias(),
			self.keyboard_sender.wrapped().clone(),
			self.hovered_keyboard_tx.clone(),
		);
	}
	fn save_state(&mut self) -> ClientState {
		ClientState::default()
	}
}
