//! `WL_TRACE=1` prints the elapsed time at named points to stderr, to explain a latency.
use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Option<Instant>> = OnceLock::new();

pub fn init() {
    START.get_or_init(|| std::env::var_os("WL_TRACE").map(|_| Instant::now()));
}

pub fn point(label: &str) {
    if let Some(Some(start)) = START.get() {
        eprintln!("wl-trace {:>8.2} ms {label}", start.elapsed().as_secs_f64() * 1e3);
    }
}
