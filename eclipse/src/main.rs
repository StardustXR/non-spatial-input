use eclipse::{input_loop, StateChange};
use std::{io::IsTerminal, sync::mpsc};

fn main() {
	if std::io::stdout().is_terminal() {
		panic!("You need to pipe this into an input sink e.g. `eclipse | azimuth`");
	}
	let (tx, rx) = mpsc::channel();
	ctrlc::set_handler(move || {
		tx.send(StateChange::Stop).unwrap();
	})
	.unwrap();
	input_loop(true, rx)
}
