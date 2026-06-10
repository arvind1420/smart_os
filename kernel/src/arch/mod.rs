/// Architecture-specific code.
///
/// Currently only x86_64 is supported. Future phases will add aarch64.

#[cfg(target_arch = "x86_64")]
pub mod x86_64;

#[cfg(target_arch = "aarch64")]
pub mod aarch64;
