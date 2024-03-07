mod handlers;

use color_eyre::Result;
use handlers::PulseReceiverCollector;
use ipc::receive_input_async_ipc;
use mint::Vector2;
use parking_lot::Mutex;
use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};
use stardust_xr_fusion::{
	client::{Client, ClientState, FrameInfo, RootHandler},
	data::{PulseReceiver, PulseSender, PulseSenderAspect},
	fields::{FieldAspect, RayMarchResult},
	node::NodeType,
	spatial::{Spatial, Transform},
};
use stardust_xr_molecules::{
	keyboard::{KeyboardEvent, KEYBOARD_MASK},
	mouse::{MouseEvent, MOUSE_MASK},
};
use std::{io::IsTerminal, sync::Arc};
use tokio::{
	sync::{watch, Notify},
	task::JoinSet,
};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PointerDatamap {
	select: f32,
	grab: f32,
	scroll_continuous: Vector2<f32>,
	scroll_discrete: Vector2<f32>,
}
impl Default for PointerDatamap {
	fn default() -> Self {
		Self {
			select: 0.0,
			grab: 0.0,
			scroll_continuous: [0.0; 2].into(),
			scroll_discrete: [0.0; 2].into(),
		}
	}
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
	if std::io::stdin().is_terminal() {
		panic!("You need to pipe azimuth or eclipse's output into this e.g. `eclipse | azimuth`");
	}
	// console_subscriber::init();
	color_eyre::install().unwrap();
	let (client, event_loop) = Client::connect_with_async_loop()
		.await
		.expect("Couldn't connect");

	// Pointer stuff
	let mouse_sender = PulseSender::create(client.get_hmd(), Transform::identity(), &MOUSE_MASK)?
		.wrap(PulseReceiverCollector::default())?;
	let (hovered_mouse_tx, hovered_mouse) = watch::channel::<Option<PulseReceiver>>(None);

	// Keyboard stuff
	let keyboard_sender =
		PulseSender::create(client.get_hmd(), Transform::identity(), &KEYBOARD_MASK)?
			.wrap(PulseReceiverCollector::default())?;
	let (hovered_keyboard_tx, hovered_keyboard) = watch::channel::<Option<PulseReceiver>>(None);

	let frame_notifier = Arc::new(Notify::new());
	let _client_root = client.wrap_root(FrameNotifier(
		frame_notifier.clone(),
		client.get_root().alias(),
	))?;

	let input_loop = tokio::task::spawn(input_loop(
		client.clone(),
		keyboard_sender.node().alias(),
		hovered_keyboard,
		mouse_sender.node().alias(),
		hovered_mouse,
	));
	tokio::task::spawn(pointer_frame_loop(
		frame_notifier.clone(),
		client.clone(),
		mouse_sender.wrapped().clone(),
		hovered_mouse_tx,
	));
	tokio::task::spawn(keyboard_frame_loop(
		frame_notifier.clone(),
		client.clone(),
		keyboard_sender.wrapped().clone(),
		hovered_keyboard_tx,
	));

	tokio::select! {
		biased;
		_ = tokio::signal::ctrl_c() => Ok(()),
		e = event_loop => e?.map_err(|e| e.into()),
		_ = input_loop => Ok(()),
	}
}

async fn input_loop(
	client: Arc<Client>,
	keyboard_sender: PulseSender,
	hovered_keyboard: watch::Receiver<Option<PulseReceiver>>,
	mouse_sender: PulseSender,
	hovered_mouse: watch::Receiver<Option<PulseReceiver>>,
) {
	let mut keymap_id: Option<String> = None;
	let mut mouse_state = MouseEvent {
		raw_input_events: Some(FxHashSet::default()),
		..Default::default()
	};

	while let Ok(message) = receive_input_async_ipc().await {
		match message {
			ipc::Message::Keymap(keymap) => {
				let Ok(future) = client.register_xkb_keymap(keymap) else {
					continue;
				};
				let Ok(new_keymap_id) = future.await else {
					continue;
				};
				keymap_id.replace(new_keymap_id);
			}
			ipc::Message::Key { keycode, pressed } => {
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
				let Some(hovered_mouse) = &*hovered_mouse.borrow() else {
					continue;
				};
				MouseEvent {
					delta: Some(delta),
					..Default::default()
				}
				.send_event(&mouse_sender, &[hovered_mouse])
			}
			ipc::Message::MouseButton { button, pressed } => {
				let Some(hovered_mouse) = &*hovered_mouse.borrow() else {
					continue;
				};
				let raw_input_events = mouse_state.raw_input_events.as_mut().unwrap();
				if pressed {
					raw_input_events.insert(button);
					&mouse_state
				} else {
					raw_input_events.remove(&button);
					&mouse_state
				}
				.send_event(&mouse_sender, &[hovered_mouse])
			}
			ipc::Message::MouseAxisContinuous(scroll) => {
				let Some(hovered_mouse) = &*hovered_mouse.borrow() else {
					continue;
				};
				MouseEvent {
					scroll_continuous: Some(scroll),
					..Default::default()
				}
				.send_event(&mouse_sender, &[hovered_mouse])
			}
			ipc::Message::MouseAxisDiscrete(scroll) => {
				let Some(hovered_mouse) = &*hovered_mouse.borrow() else {
					continue;
				};
				MouseEvent {
					scroll_discrete: Some(scroll),
					..Default::default()
				}
				.send_event(&mouse_sender, &[hovered_mouse])
			}
			ipc::Message::Disconnect => break,
		}
	}
}

async fn pointer_frame_loop(
	frame_notifier: Arc<Notify>,
	client: Arc<Client>,
	mouse_sender: Arc<Mutex<PulseReceiverCollector>>,
	hovered_mouse_tx: watch::Sender<Option<PulseReceiver>>,
) {
	loop {
		frame_notifier.notified().await;
		detect_hover(&client, mouse_sender.clone(), &hovered_mouse_tx).await
	}
}

async fn keyboard_frame_loop(
	frame_notifier: Arc<Notify>,
	client: Arc<Client>,
	keyboard_sender: Arc<Mutex<PulseReceiverCollector>>,
	hovered_keyboard_tx: watch::Sender<Option<PulseReceiver>>,
) {
	loop {
		frame_notifier.notified().await;
		detect_hover(&client, keyboard_sender.clone(), &hovered_keyboard_tx).await
	}
}

async fn detect_hover(
	client: &Client,
	sender: Arc<Mutex<PulseReceiverCollector>>,
	hovered_tx: &watch::Sender<Option<PulseReceiver>>,
) {
	let mut closest_hit: Option<(PulseReceiver, RayMarchResult)> = None;
	let mut join = JoinSet::new();
	for (receiver, field) in sender.lock().0.values() {
		let receiver = receiver.alias();
		let field = field.alias();
		let hmd = client.get_hmd().alias();
		join.spawn(async move {
			(
				receiver,
				field.ray_march(&hmd, [0.0; 3], [0.0, 0.0, -1.0]).await,
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
	let _ = hovered_tx.send(closest_hit.map(|(r, _)| r));
}

struct FrameNotifier(Arc<Notify>, Spatial);
impl RootHandler for FrameNotifier {
	fn frame(&mut self, _info: FrameInfo) {
		self.0.notify_waiters();
	}
	fn save_state(&mut self) -> ClientState {
		ClientState::from_root(&self.1)
	}
}
