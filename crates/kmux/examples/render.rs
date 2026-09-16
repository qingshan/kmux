//! Dev tool: read a captured tmux control-mode stream on stdin, parse it
//! (control protocol + VT) and print the rendered screen. Use to validate
//! the pipeline against real tmux output: `cat stream.bin | cargo run --example render`.
use kmuxd::control::{ControlEvent, ControlParser};
use kmuxd::screen::Screen;
use std::io::Read;

fn main() {
    let mut bytes = Vec::new();
    std::io::stdin().read_to_end(&mut bytes).unwrap();
    let mut parser = ControlParser::new();
    let mut events = Vec::new();
    parser.feed(&bytes, &mut events);
    let mut screen = Screen::new(80, 24);
    for ev in events {
        match ev {
            ControlEvent::Block(t) => screen.feed(t.as_bytes()),
            ControlEvent::Output { data, .. } => screen.feed(&data),
            _ => {}
        }
    }
    let (text, _attrs) = screen.render();
    for (i, row) in text.iter().enumerate() {
        println!("{:2}|{}", i, row);
    }
}
