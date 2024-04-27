use input_window::InputWindow;
use std::{io::IsTerminal, process::exit};
use winit::event_loop::ControlFlow;
use winit::event_loop::EventLoopBuilder;
pub mod input_window;

fn main() {
	if std::io::stdout().is_terminal() {
		panic!("You need to pipe this into an input sink e.g. `manifold | azimuth`");
	}
	ctrlc::set_handler(|| exit(0)).unwrap();
	let event_loop = EventLoopBuilder::new().build().unwrap();
	let mut input_window = InputWindow::new(&event_loop);

	event_loop
		.run(move |event, elwt| {
			elwt.set_control_flow(ControlFlow::Wait);
			input_window.handle_event(event, elwt);
		})
		.unwrap();
}
