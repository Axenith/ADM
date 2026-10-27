//! ADM download engine: segmented, resumable HTTP(S) downloads.
//!
//! ```no_run
//! # async fn demo() {
//! use adm_core::*;
//! let engine = Engine::new(EngineConfig::new("/tmp/adm-state")).unwrap();
//! let req = DownloadRequest::new(1, "https://example.com/big.iso", "/tmp/downloads");
//! let (handle, mut events) = engine.start(req);
//! while let Ok(ev) = events.recv().await {
//!     if let Event::Progress(p) = ev { println!("{} bytes at {:.0} B/s", p.downloaded, p.speed); }
//! }
//! match handle.wait().await { Outcome::Completed(f) => println!("{:?}", f.path), _ => {} }
//! # }
//! ```

mod coord;
mod engine;
mod error;
mod limiter;
mod model;
mod progress;
mod segment;
mod store;
mod util;

pub use engine::{DownloadHandle, Engine};
pub use error::{DownloadError, Result};
pub use limiter::SpeedLimiter;
pub use model::*;
pub use util::sanitize_file_name;
