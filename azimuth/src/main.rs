use glam::{Quat, Vec3};
use input_event_codes::{BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
use ipc::receive_input_async_ipc;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use spatializer::spatial_input_beam;
use stardust_xr_fusion::{
	client::{Client, ClientHandle},
	core::{
		schemas::zbus::Connection,
		values::{color::rgba_linear, Datamap, Vector2},
	},
	drawable::Lines,
	fields::{Field, FieldRefAspect, RayMarchResult},
	input::{
		InputDataType, InputHandler, InputMethod, InputMethodAspect, InputMethodEvent, Pointer,
	},
	node::NodeType,
	objects::hmd,
	root::{ClientState, RootAspect, RootEvent},
	spatial::{SpatialAspect, SpatialRef, Transform},
	AsyncEventHandle,
};
use stardust_xr_molecules::{
	keyboard::KeyboardHandlerProxy,
	lines::{circle, LineExt},
};
use std::{f32::consts::FRAC_PI_2, io::IsTerminal, sync::Arc};
use tokio::{
	sync::{mpsc, watch, Notify},
	task::JoinSet,
};
use tracing::{debug_span, info, Instrument};

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

enum MouseEvent {
	Move { delta: Vector2<f32> },
	Button { button: u32, pressed: bool },
	AxisContinuous { a: Vector2<f32> },
	AxisDiscrete { a: Vector2<f32> },
}

enum KeyboardEvent {
	Key { map: u64, key: u32, pressed: bool },
	KeyMap(u64),
}

#[tokio::main]
async fn main() {
	if std::io::stdin().is_terminal() {
		panic!("You need to pipe manifold or eclipse's output into this e.g. `eclipse | azimuth`");
	}
	color_eyre::install().unwrap();

	// Client setup
	let client = Client::connect().await.expect("Couldn't connect");
	let client_handle = client.handle();
	let async_loop = client.async_event_loop();
	let hmd = hmd(&client_handle).await.unwrap();

	let dbus_connection = Connection::session().await.unwrap();

	// Setup the visual pointer and reticle
	let pointer = InputMethod::create(
		client_handle.get_root(),
		Transform::identity(),
		InputDataType::Pointer(Pointer {
			origin: [0.0; 3].into(),
			orientation: Quat::IDENTITY.into(),
			deepest_point: [0.0; 3].into(),
		}),
		&Datamap::from_typed(PointerDatamap::default()).unwrap(),
	)
	.unwrap();
	let _ = pointer.set_relative_transform(&hmd, Transform::from_translation([0.0; 3]));

	// Create the visual reticle
	let line = circle(8, 0.0, 0.001)
		.thickness(0.0025)
		.color(rgba_linear!(1.0, 1.0, 1.0, 1.0));
	let pointer_reticle = Lines::create(
		&pointer,
		Transform::from_translation_rotation([0.0, 0.0, -0.5], Quat::from_rotation_x(FRAC_PI_2)),
		&[line],
	)
	.unwrap();

	// Event handling setup
	let frame_event = Arc::new(Notify::new());
	let (keyboard_tx, keyboard_rx) = mpsc::unbounded_channel::<KeyboardEvent>();
	let (mouse_tx, mouse_rx) = mpsc::unbounded_channel::<MouseEvent>();
	let (frame_count_tx, frame_count_rx) = watch::channel(0);

	// Spawn the main task loops
	let frame_loop = tokio::task::spawn(handle_frame_events(
		frame_event.clone(),
		client_handle.clone(),
		async_loop.get_event_handle(),
		pointer.clone(),
		hmd.clone(),
		frame_count_tx.clone(),
	));

	let mouse_loop = tokio::task::spawn(handle_mouse_events(
		pointer.clone(),
		mouse_rx,
		frame_event.clone(),
		frame_count_rx.clone(),
	));

	let (state_tx, state_rx) = watch::channel(MouseTargetState::default());

	let input_method_events = tokio::task::spawn(input_method_events(
		async_loop.get_event_handle(),
		pointer.clone(),
		state_tx,
	));

	let input_method_loop = tokio::task::spawn(input_method_loop(
		frame_event.clone(),
		state_rx,
		pointer.clone(),
		pointer_reticle,
	));

	let keyboard_loop =
		tokio::task::spawn(
			spatial_input_beam::<KeyboardHandlerProxy, KeyboardEvent, ()>(
				dbus_connection,
				pointer.clone().as_spatial().as_spatial_ref(),
				keyboard_rx,
				async |proxy, event, _| match event {
					KeyboardEvent::KeyMap(keymap_id) => {
						_ = proxy
							.keymap(keymap_id)
							.instrument(debug_span!("sending keymap"))
							.await;
					}
					KeyboardEvent::Key { key, pressed, map } => {
						_ = proxy
							.keymap(map)
							.instrument(debug_span!("sending keymap as part of button"))
							.await;
						_ = proxy
							.key_state(key, pressed)
							.instrument(debug_span!("sending keypress"))
							.await;
					}
				},
				async |_, _| {},
				async |proxy| _ = proxy.reset().await,
			),
		);

	let input_loop = tokio::task::spawn(input_loop(client_handle.clone(), keyboard_tx, mouse_tx));

	tokio::select! {
		biased;
		_ = tokio::signal::ctrl_c() => (),
		_ = input_method_events => (),
		_ = input_method_loop => (),
		_ = mouse_loop => (),
		_ = keyboard_loop => (),
		_ = input_loop => (),
		_ = frame_loop => (),
	}
}

async fn handle_mouse_events(
	pointer: InputMethod,
	mut mouse_rx: mpsc::UnboundedReceiver<MouseEvent>,
	event_handle: Arc<Notify>,
	frame_count_rx: watch::Receiver<u32>,
) {
	let mut yaw = 0.0;
	let mut pitch = 0.0;
	let mut pointer_datamap = PointerDatamap::default();
	let mut old_frame_count = 0_u32;
	let mut mouse_buttons = FxHashSet::default();

	loop {
		event_handle.notified().await;

		if *frame_count_rx.borrow() > old_frame_count {
			old_frame_count = *frame_count_rx.borrow();
			pointer_datamap.scroll_continuous = [0.0; 2].into();
			pointer_datamap.scroll_discrete = [0.0; 2].into();
		}

		while let Ok(event) = mouse_rx.try_recv() {
			match event {
				MouseEvent::Move { delta } => {
					yaw += delta.x * MOUSE_SENSITIVITY;
					pitch += delta.y * MOUSE_SENSITIVITY;
					pitch = pitch.clamp(-90.0, 90.0);

					let rotation_x = Quat::from_rotation_x(-pitch.to_radians());
					let rotation_y = Quat::from_rotation_y(-yaw.to_radians());
					let _ = pointer
						.set_local_transform(Transform::from_rotation(rotation_y * rotation_x));
				}
				MouseEvent::Button { button, pressed } => {
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
							pointer_datamap.select = pressed as u32 as f32;
						}
						BTN_MIDDLE!() => {
							pointer_datamap.middle = pressed as u32 as f32;
						}
						BTN_RIGHT!() => {
							pointer_datamap.context = pressed as u32 as f32;
							pointer_datamap.grab = pressed as u32 as f32;
						}
						_ => {}
					}
				}
				MouseEvent::AxisContinuous { a } => {
					pointer_datamap.scroll_continuous.x += a.x;
					pointer_datamap.scroll_continuous.y += a.y;
				}
				MouseEvent::AxisDiscrete { a } => {
					pointer_datamap.scroll_discrete.x += a.x;
					pointer_datamap.scroll_discrete.y += a.y;
				}
			}
		}
		dbg!(&pointer_datamap);
		let _ = pointer.set_datamap(&Datamap::from_typed(pointer_datamap.clone()).unwrap());
	}
}

#[derive(Clone, Default)]
struct MouseTargetState {
	handlers: FxHashMap<u64, (InputHandler, Field)>,
	capture_requests: FxHashSet<u64>,
	captured: Option<u64>,
}

async fn input_method_events(
	async_event_handle: AsyncEventHandle,
	pointer: InputMethod,
	state_tx: watch::Sender<MouseTargetState>,
) {
	let mut state = MouseTargetState::default();

	loop {
		async_event_handle.wait().await;
		let mut state_changed = false;

		while let Some(event) = pointer.recv_input_method_event() {
			state_changed = true;
			match event {
				InputMethodEvent::CreateHandler { handler, field } => {
					state.handlers.insert(handler.id(), (handler, field));
				}
				InputMethodEvent::RequestCaptureHandler { id } => {
					state.capture_requests.insert(id);
				}
				InputMethodEvent::DestroyHandler { id } => {
					state.handlers.remove(&id);
				}
			}
		}

		if state_changed {
			let _ = state_tx.send(state.clone());
		}
	}
}

async fn input_method_loop(
	frame_event: Arc<Notify>,
	state_rx: watch::Receiver<MouseTargetState>,
	pointer: InputMethod,
	pointer_reticle: Lines,
) {
	loop {
		frame_event.notified().await;

		let mut state = state_rx.borrow().clone();

		if let Some(captured_id) = state.captured {
			if !state.capture_requests.contains(&captured_id) {
				state.captured = None;
			}
		}
		if state.captured.is_none() {
			state.captured = state.capture_requests.drain().next();
		}
		if let Some((captured, _)) = state.captured.and_then(|id| state.handlers.get(&id)) {
			pointer.set_handler_order(&[captured.clone()]).unwrap();
			pointer.set_captures(&[captured.clone()]).unwrap();
			continue;
		}
		let _ = pointer.set_captures(&[]);

		let mut join = JoinSet::new();
		for (handler, field) in state.handlers.values() {
			let handler = handler.clone();
			let field = field.clone();
			let pointer = pointer.clone();
			join.spawn(async move {
				(
					handler,
					field.ray_march(&pointer, [0.0; 3], [0.0, 0.0, -1.0]).await,
				)
			});
		}

		let mut handlers: Vec<(InputHandler, RayMarchResult)> = Vec::new();
		while let Some(res) = join.join_next().await {
			let Ok((handler, Ok(ray_info))) = res else {
				continue;
			};
			if ray_info.min_distance > 0.0 {
				continue;
			}
			if ray_info.deepest_point_distance < 0.01 {
				continue;
			}
			handlers.push((handler, ray_info));
		}
		let closest_hits = handlers
			.into_iter()
			.map(|(a, b)| (vec![a], b))
			// now collect all handlers that are same distance if they're the closest
			.reduce(|(mut handlers_a, result_a), (handlers_b, result_b)| {
				if (result_a.deepest_point_distance - result_b.deepest_point_distance).abs() < 0.001
				{
					// distance is basically the same
					handlers_a.extend(handlers_b);
					(handlers_a, result_a)
				} else if result_a.deepest_point_distance < result_b.deepest_point_distance {
					(handlers_a, result_a)
				} else {
					(handlers_b, result_b)
				}
			});

		if let Some((hit_handlers, hit_info)) = closest_hits {
			let _ = pointer.set_handler_order(hit_handlers.as_slice());
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
	}
}

// Keyboard events are now handled directly by spatial_input_beam
async fn input_loop(
	client: Arc<ClientHandle>,
	keyboard_tx: mpsc::UnboundedSender<KeyboardEvent>,
	mouse_tx: mpsc::UnboundedSender<MouseEvent>,
) {
	let mut keymap = None;

	while let Ok(message) = receive_input_async_ipc()
		.instrument(debug_span!("handling input ipc message"))
		.await
	{
		match message {
			ipc::Message::Keymap(map) => {
				info!("IPC keymap message");
				let Ok(future) = client.register_xkb_keymap(map) else {
					continue;
				};
				let Ok(new_keymap_id) = future.await else {
					continue;
				};
				keymap = Some(new_keymap_id);
				let _ = keyboard_tx.send(KeyboardEvent::KeyMap(new_keymap_id));
			}
			ipc::Message::Key { keycode, pressed } => {
				let Some(map) = keymap else {
					continue;
				};
				let _ = keyboard_tx.send(KeyboardEvent::Key {
					map,
					key: keycode,
					pressed,
				});
			}
			ipc::Message::MouseMove(delta) => {
				let _ = mouse_tx.send(MouseEvent::Move { delta });
			}
			ipc::Message::MouseButton { button, pressed } => {
				let _ = mouse_tx.send(MouseEvent::Button { button, pressed });
			}
			ipc::Message::MouseAxisContinuous(a) => {
				let _ = mouse_tx.send(MouseEvent::AxisContinuous { a });
			}
			ipc::Message::MouseAxisDiscrete(a) => {
				let _ = mouse_tx.send(MouseEvent::AxisDiscrete { a });
			}
			ipc::Message::ResetInput => {}
			ipc::Message::Disconnect => break,
		}
	}
}

async fn handle_frame_events(
	frame_handle: Arc<Notify>,
	client_handle: Arc<ClientHandle>,
	async_event_handle: AsyncEventHandle,
	pointer: InputMethod,
	hmd: SpatialRef,
	frame_count_tx: watch::Sender<u32>,
) {
	loop {
		async_event_handle.wait().await;
		match client_handle.get_root().recv_root_event() {
			Some(RootEvent::Frame { info: _ }) => {
				frame_count_tx.send_modify(|i| *i += 1);
				let _ = pointer.set_relative_transform(&hmd, Transform::from_translation([0.0; 3]));
				frame_handle.notify_waiters();
			}
			Some(RootEvent::Ping { response }) => {
				response.send(Ok(()));
			}
			Some(RootEvent::SaveState { response }) => {
				response.send(ClientState::from_root(client_handle.get_root()));
			}
			None => {}
		}
	}
}
