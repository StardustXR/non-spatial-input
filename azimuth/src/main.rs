#![allow(clippy::mutable_key_type)]

use glam::Quat;
use gluon::{Interface, Liveness};
use input_event_codes::{BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
use ipc::receive_input_async_ipc;
use parking_lot::Mutex;
use stardust_xr_fusion::{
	client::{Client, FrameInfo},
	drawable::{Line, Lines, LinesExt},
	fields::RayMarchResult,
	keymap::{KeymapStore, KeymapStoreExt},
	query::QueryableId,
	spatial::{PartialTransform, Spatial, SpatialExt, SpatialRef, Transform},
	suis::{DatamapData, InputDataType, InputHandler, Pointer},
	tracked::{Tracked, TrackedExt},
	types::{rgba_linear, Color, Posef, Timestamp, Vec2F},
};
use stardust_xr_molecules::{
	input_method::{CachedHandler, DatamapBuilder, InputMethod, InputMethodHelper},
	keyboard_handler::{
		protocol::{KeyEvent, KeyboardHandler},
		ModifierState,
	},
	lines::{circle, LineExt},
	spatial_input_beam::SpatialInputBeam,
};
use std::{
	collections::{HashMap, HashSet},
	f32::consts::FRAC_PI_2,
	io::IsTerminal,
	sync::Arc,
};
use tokio::sync::broadcast::{self, error::RecvError};
use tracing::{debug_span, warn, Instrument};
use tracing_subscriber::{layer::SubscriberExt as _, EnvFilter};

const MOUSE_SENSITIVITY: f32 = 0.1;
const RETICLE_REST: f32 = 0.5;
/// how far off a field the beam still reports, which is the range the reticle blends over
const BEAM_MARGIN: f32 = 0.1;
/// sit just short of the deepest point so the reticle reads as on top of what it's over
const RETICLE_LIFT: f32 = 0.95;
/// how sharply a near miss loses its pull as it slides out to the margin
const FALLOFF: i32 = 2;
const IDLE: Color = rgba_linear!(1.0, 1.0, 1.0, 1.0);
const CAPTURED: Color = rgba_linear!(0.0, 1.0, 0.0, 1.0);

fn reticle(color: Color) -> Vec<Line> {
	vec![circle(8, 0.0, 0.001).thickness(0.0025).color(color)]
}

#[derive(Default)]
struct MouseState {
	yaw: f32,
	pitch: f32,

	select: f32,
	middle: f32,
	context: f32,
	grab: f32,

	scroll_continuous: [f32; 2],
	scroll_discrete: [f32; 2],
}

struct MousePointer {
	spatial: Spatial,
	state: Mutex<MouseState>,
}
impl MousePointer {
	fn look(&self, delta: Vec2F) {
		let mut state = self.state.lock();
		state.yaw += delta.x * MOUSE_SENSITIVITY;
		state.pitch = (state.pitch - delta.y * MOUSE_SENSITIVITY).clamp(-90.0, 90.0);
		let rotation = Quat::from_rotation_y(-state.yaw.to_radians())
			* Quat::from_rotation_x(-state.pitch.to_radians());
		drop(state);

		let _ = self
			.spatial
			.set_local_transform(PartialTransform::from_rotation(rotation));
	}

	fn button(&self, button: u32, pressed: bool) {
		let pressed = pressed as u32 as f32;
		let mut state = self.state.lock();
		match button {
			BTN_LEFT!() => state.select = pressed,
			BTN_MIDDLE!() => state.middle = pressed,
			BTN_RIGHT!() => {
				state.context = pressed;
				state.grab = pressed;
			}
			_ => {}
		}
	}

	fn scroll(&self, continuous: Option<Vec2F>, discrete: Option<Vec2F>) {
		let mut state = self.state.lock();
		if let Some(a) = continuous {
			state.scroll_continuous[0] += a.x;
			state.scroll_continuous[1] += a.y;
		}
		if let Some(a) = discrete {
			state.scroll_discrete[0] += a.x;
			state.scroll_discrete[1] += a.y;
		}
	}

	/// scroll is a delta, so it only counts for the frame it arrived in
	fn end_frame(&self) {
		let mut state = self.state.lock();
		state.scroll_continuous = [0.0; 2];
		state.scroll_discrete = [0.0; 2];
	}
}
impl InputMethodHelper for MousePointer {
	type QueryValue = RayMarchResult;

	async fn order_handlers_and_captures(
		&self,
		handlers: &HashMap<QueryableId, CachedHandler<RayMarchResult>>,
		capture_requests: &HashSet<InputHandler>,
	) -> (Vec<InputHandler>, Option<InputHandler>) {
		// a capture requester is picked out of every handler the query knows, not just the ones
		// the beam is currently on, so looking away from what you grabbed doesn't drop it
		let capture = closest(
			handlers
				.values()
				.filter(|e| e.spatial.is_some() && capture_requests.contains(&e.handler)),
		);
		if let Some(handler) = capture {
			return (vec![handler.clone()], Some(handler));
		}
		(hits(handlers).into_iter().map(|(_, h)| h).collect(), None)
	}

	async fn input_data(&self, _time: Timestamp) -> Option<InputDataType> {
		Some(InputDataType::Pointer {
			data: Pointer {
				pose: Posef::default(),
				deepest_point: 0.0,
			},
		})
	}

	async fn datamap(&self) -> HashMap<String, DatamapData> {
		let state = self.state.lock();
		DatamapBuilder::default()
			.bool("mouse", true)
			.f32("select", state.select)
			.f32("middle", state.middle)
			.f32("context", state.context)
			.f32("grab", state.grab)
			.vec2("scroll_continuous", state.scroll_continuous)
			.vec2("scroll_discrete", state.scroll_discrete)
			.build()
	}
}

/// handlers the beam is actually inside, nearest first
fn hits(
	handlers: &HashMap<QueryableId, CachedHandler<RayMarchResult>>,
) -> Vec<(f32, InputHandler)> {
	let mut hits: Vec<(f32, InputHandler)> = handlers
		.values()
		.filter(|e| e.spatial.is_some())
		// a positive min_distance is a near miss, and anything at the origin is the client's own
		.filter(|e| e.value.min_distance <= 0.0 && e.value.deepest_point_distance >= 0.01)
		.map(|e| (e.value.deepest_point_distance, e.handler.clone()))
		.collect();
	hits.sort_by(|(a, _), (b, _)| a.total_cmp(b));
	hits
}

/// where the reticle wants to sit down the beam
///
/// landing on something locks it to that depth. Short of that, everything inside the margin
/// pulls it toward its own depth by how near the beam comes, so it drifts onto what you're
/// closing in on rather than hanging at rest until you're already on top of it
fn reticle_depth(handlers: &HashMap<QueryableId, CachedHandler<RayMarchResult>>) -> f32 {
	if let Some((depth, _)) = hits(handlers).first() {
		return depth * RETICLE_LIFT;
	}

	let mut weight = 0.0;
	let mut weighted_depth = 0.0;
	for entry in handlers.values().filter(|e| e.spatial.is_some()) {
		// falls to zero at the margin rather than merely getting small, so nothing pops into
		// the average the instant the query picks it up
		let w = (1.0 - entry.value.min_distance / BEAM_MARGIN)
			.clamp(0.0, 1.0)
			.powi(FALLOFF);
		weight += w;
		weighted_depth += w * entry.value.deepest_point_distance;
	}
	if weight == 0.0 {
		return RETICLE_REST;
	}

	let blended = weighted_depth / weight * RETICLE_LIFT;
	RETICLE_REST + (blended - RETICLE_REST) * weight.min(1.0)
}

fn closest<'a>(
	handlers: impl Iterator<Item = &'a CachedHandler<RayMarchResult>>,
) -> Option<InputHandler> {
	handlers
		.min_by(|a, b| {
			a.value
				.deepest_point_distance
				.total_cmp(&b.value.deepest_point_distance)
		})
		.map(|e| e.handler.clone())
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
	if std::io::stdin().is_terminal() {
		panic!("You need to pipe manifold or eclipse's output into this e.g. `eclipse | azimuth`");
	}
	tracing::subscriber::set_global_default(
		tracing_subscriber::registry()
			.with(EnvFilter::from_default_env())
			.with(tracing_subscriber::fmt::layer().compact()),
	)
	.unwrap();

	let (client, _root) = Client::connect(&[]).await.expect("Couldn't connect");
	let hmd = Tracked::hmd_spatial().await.unwrap();

	// parented to the root, not the hmd, so only the mouse ever rotates it, the frame loop
	// pins its position to the hmd
	let (pointer_spatial, pointer_ref) = Spatial::new(&client, client.root(), Transform::IDENTITY)
		.await
		.unwrap();
	// circle() is on the XZ plane, stand it up so it faces down the beam
	let (reticle_spatial, _) = Spatial::new(
		&client,
		&pointer_ref,
		Transform::from_translation_rotation(
			[0.0, 0.0, -RETICLE_REST],
			Quat::from_rotation_x(FRAC_PI_2),
		),
	)
	.await
	.unwrap();
	let reticle_lines = Lines::new(&client, &reticle_spatial, reticle(IDLE))
		.await
		.unwrap();

	let (method, _proxy, _query) = InputMethod::new_beam(
		&client,
		MousePointer {
			spatial: pointer_spatial,
			state: Mutex::default(),
		},
		pointer_ref.clone(),
		[0.0; 3].into(),
		[0.0, 0.0, -1.0].into(),
		f32::INFINITY,
		BEAM_MARGIN,
	)
	.await
	.unwrap();

	let keyboard_beam = SpatialInputBeam::new(
		&client,
		pointer_ref,
		|_, v| Some(KeyboardHandler::from_ref(v)),
		KeyboardHandler::ID.into(),
		f32::INFINITY,
		// keys go to whatever the beam is truly on, no near-miss fuzz
		0.0,
	)
	.await
	.unwrap();

	let keymap_store = KeymapStore::connect().await.unwrap();
	let input_loop = tokio::task::spawn(input_loop(
		keymap_store,
		method.handler().clone(),
		keyboard_beam.handler().clone(),
	));
	let frame_loop = tokio::task::spawn(frame_loop(
		client.frame_receiver(),
		method.handler().clone(),
		hmd,
		reticle_spatial,
		reticle_lines,
	));

	tokio::select! {
		biased;
		_ = client.server().death_notification() => (),
		_ = tokio::signal::ctrl_c() => (),
		_ = input_loop => (),
		_ = frame_loop => (),
	}
	drop(method);
	drop(keyboard_beam);
}

async fn frame_loop(
	mut frames: broadcast::Receiver<FrameInfo>,
	method: Arc<InputMethod<MousePointer>>,
	hmd: SpatialRef,
	reticle_spatial: Spatial,
	reticle_lines: Lines,
) {
	let mut captured = false;
	let mut distance = RETICLE_REST;
	loop {
		let info = match frames.recv().await {
			Ok(info) => info,
			Err(RecvError::Lagged(n)) => {
				warn!("lost {n} frame events");
				continue;
			}
			Err(RecvError::Closed) => break,
		};

		// translation only, so the pointer rides along with the head without turning with it
		let _ = method
			.spatial
			.set_relative_transform(hmd.clone(), PartialTransform::from_translation([0.0; 3]));

		method.send(info.predicted_display_time).await;
		method.end_frame();

		let now_captured = method.active_capture().await.is_some();
		if now_captured != captured {
			captured = now_captured;
			let _ = reticle_lines.set_lines(reticle(if captured { CAPTURED } else { IDLE }));
		}

		let now_distance = reticle_depth(&*method.cache().handlers().await);
		if now_distance != distance {
			distance = now_distance;
			let _ = reticle_spatial
				.set_local_transform(PartialTransform::from_translation([0.0, 0.0, -distance]));
		}
	}
}

async fn input_loop(
	keymap_store: KeymapStore,
	pointer: Arc<InputMethod<MousePointer>>,
	keyboard_beam: Arc<SpatialInputBeam<KeyboardHandler>>,
) {
	let mut keymap = None;

	while let Ok(message) = receive_input_async_ipc()
		.instrument(debug_span!("handling input ipc message"))
		.await
	{
		match message {
			ipc::Message::Keymap(map) => {
				let Some(Ok(new_keymap)) = keymap_store
					.exchange_string(&map)
					.await
					.map(|v| v.inspect_err(|err| tracing::error!("failed keymap exchange: {err}")))
				else {
					warn!("failed keymap exchange");
					continue;
				};
				keymap = Some(new_keymap);
			}
			ipc::Message::Key {
				keycode,
				pressed,
				mod_pressed,
				mod_latched,
				mod_locked,
				layout_group,
			} => {
				let Some(keymap) = keymap.clone() else {
					warn!("no keymap");
					continue;
				};
				let Some(handler) = keyboard_beam.get_handler().await else {
					continue;
				};
				let _ = handler
					.key(
						KeyEvent {
							keycode,
							pressed,
							modifiers: ModifierState {
								depressed: mod_pressed,
								latched: mod_latched,
								locked: mod_locked,
								layout_group,
							},
							keymap,
						},
						None,
					)
					.instrument(debug_span!("sending keypress"));
			}
			ipc::Message::MouseMove(delta) => pointer.look(delta),
			ipc::Message::MouseButton { button, pressed } => pointer.button(button, pressed),
			ipc::Message::MouseAxisContinuous(a, _) => pointer.scroll(Some(a), None),
			ipc::Message::MouseAxisDiscrete(a, _) => pointer.scroll(None, Some(a)),
			ipc::Message::ResetInput => {}
			ipc::Message::Disconnect => break,
		}
	}
}
