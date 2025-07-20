use candid::Principal;
use rand::Rng;

pub mod context;
pub mod engine;
pub mod extension;
pub mod management;
pub mod memory;
pub mod model;
pub mod store;

/// Gets current unix timestamp in milliseconds
pub use structured_logger::unix_ms;

/// Generates N random bytes
pub use ic_cose::rand_bytes;

/// This is used to represent unauthenticated or anonymous users in the system.
pub const ANONYMOUS: Principal = Principal::anonymous();

pub static APP_USER_AGENT: &str = concat!(
    "Mozilla/5.0 anda.bot ",
    env!("CARGO_PKG_NAME"),
    "/",
    env!("CARGO_PKG_VERSION"),
);

/// Generates a random number within the given range
pub fn rand_number<T, R>(range: R) -> T
where
    T: rand::distr::uniform::SampleUniform,
    R: rand::distr::uniform::SampleRange<T>,
{
    let mut rng = rand::rng();
    rng.random_range(range)
}
