//! Prints which agent (and session) runs below each given shell process.
//!
//! Usage: `cargo run -p termroost-core --example detect_agents -- <shell pid>...`

use termroost_core::PaneId;
use termroost_core::detect::{AgentHomes, Detector};

fn main() {
    let shells: Vec<(PaneId, u32)> =
        std::env::args().skip(1).enumerate().filter_map(|(i, a)| Some((PaneId(i as i64), a.parse().ok()?))).collect();
    let mut detector = Detector::new(AgentHomes::from_env());
    let found = detector.scan(&shells);
    for (pane, shell) in &shells {
        match found.iter().find(|(p, _)| p == pane).and_then(|(_, a)| a.as_ref()) {
            Some(a) => println!("shell {shell}: {} (pid {}) session {:?}", a.kind.label(), a.pid, a.session_id),
            None => println!("shell {shell}: no agent"),
        }
    }
}
