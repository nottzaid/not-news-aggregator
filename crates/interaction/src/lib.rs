//! Platform-neutral Not News canvas interaction state.
//!
//! The desktop app remains the source of truth while the browser port is
//! proven. Including that source here guarantees both front ends execute the
//! same hit testing, hover expansion, pan, zoom, drag, and timing behavior.

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../app/src/interaction.rs"
));
