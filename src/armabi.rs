//! Functions the C library of the soft float ARM build lacks, which the cross
//! toolchain of Rust and zig expects from it.
//!
//! The C23 functions `fmaximum_num` and `fminimum_num`, to which Rust lowers
//! `f32::max` and friends there. They return the other operand when one is NaN
//! and order -0 below +0. The bodies must not call `max` or `min`, which would
//! call these functions again.
//!
//! The ARM run-time ABI helpers `__aeabi_uread4` and friends, which read and
//! write integers at unaligned addresses; the standard library calls them.
//! They move one byte at a time through volatile accesses, which the compiler
//! cannot merge into the unaligned access that would call them again.

fn maximum(a: f64, b: f64) -> f64 {
    if a.is_nan() || b > a || (b == a && a.is_sign_negative()) {
        b
    } else {
        a
    }
}

fn minimum(a: f64, b: f64) -> f64 {
    if a.is_nan() || b < a || (b == a && b.is_sign_negative()) {
        b
    } else {
        a
    }
}

#[cfg(all(target_arch = "arm", target_env = "musl"))]
#[no_mangle]
pub extern "C" fn fmaximum_num(a: f64, b: f64) -> f64 {
    maximum(a, b)
}

#[cfg(all(target_arch = "arm", target_env = "musl"))]
#[no_mangle]
pub extern "C" fn fminimum_num(a: f64, b: f64) -> f64 {
    minimum(a, b)
}

#[cfg(all(target_arch = "arm", target_env = "musl"))]
#[no_mangle]
pub extern "C" fn fmaximum_numf(a: f32, b: f32) -> f32 {
    maximum(a.into(), b.into()) as f32
}

#[cfg(all(target_arch = "arm", target_env = "musl"))]
#[no_mangle]
pub extern "C" fn fminimum_numf(a: f32, b: f32) -> f32 {
    minimum(a.into(), b.into()) as f32
}

fn read_bytes<const N: usize>(p: *const u8) -> [u8; N] {
    let mut bytes = [0; N];
    for (i, byte) in bytes.iter_mut().enumerate() {
        // SAFETY: the caller hands an address of N readable bytes.
        *byte = unsafe { core::ptr::read_volatile(p.add(i)) };
    }
    bytes
}

fn write_bytes(p: *mut u8, bytes: &[u8]) {
    for (i, byte) in bytes.iter().enumerate() {
        // SAFETY: the caller hands an address of as many writable bytes.
        unsafe { core::ptr::write_volatile(p.add(i), *byte) };
    }
}

/// # Safety
///
/// `p` addresses 4 readable bytes.
#[cfg(all(target_arch = "arm", target_env = "musl"))]
#[no_mangle]
pub unsafe extern "C" fn __aeabi_uread4(p: *const u8) -> u32 {
    u32::from_ne_bytes(read_bytes(p))
}

/// # Safety
///
/// `p` addresses 4 writable bytes.
#[cfg(all(target_arch = "arm", target_env = "musl"))]
#[no_mangle]
pub unsafe extern "C" fn __aeabi_uwrite4(value: u32, p: *mut u8) -> u32 {
    write_bytes(p, &value.to_ne_bytes());
    value
}

/// # Safety
///
/// `p` addresses 8 readable bytes.
#[cfg(all(target_arch = "arm", target_env = "musl"))]
#[no_mangle]
pub unsafe extern "C" fn __aeabi_uread8(p: *const u8) -> u64 {
    u64::from_ne_bytes(read_bytes(p))
}

/// # Safety
///
/// `p` addresses 8 writable bytes.
#[cfg(all(target_arch = "arm", target_env = "musl"))]
#[no_mangle]
pub unsafe extern "C" fn __aeabi_uwrite8(value: u64, p: *mut u8) -> u64 {
    write_bytes(p, &value.to_ne_bytes());
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn they_follow_c23() {
        assert_eq!(maximum(1.0, 2.0), 2.0);
        assert_eq!(minimum(1.0, 2.0), 1.0);
        assert_eq!(maximum(f64::NAN, 2.0), 2.0);
        assert_eq!(maximum(2.0, f64::NAN), 2.0);
        assert_eq!(minimum(f64::NAN, 2.0), 2.0);
        assert!(maximum(f64::NAN, f64::NAN).is_nan());
        assert!(maximum(-0.0, 0.0).is_sign_positive());
        assert!(maximum(0.0, -0.0).is_sign_positive());
        assert!(minimum(-0.0, 0.0).is_sign_negative());
        assert!(minimum(0.0, -0.0).is_sign_negative());
    }

    #[test]
    fn unaligned_bytes_round_trip() {
        let mut buf = [0u8; 13];
        write_bytes(buf.as_mut_ptr().wrapping_add(1), &0x1122_3344_5566_7788u64.to_ne_bytes());
        assert_eq!(u64::from_ne_bytes(read_bytes(buf.as_ptr().wrapping_add(1))), 0x1122_3344_5566_7788);
        write_bytes(buf.as_mut_ptr().wrapping_add(9), &0xdead_beefu32.to_ne_bytes());
        assert_eq!(u32::from_ne_bytes(read_bytes(buf.as_ptr().wrapping_add(9))), 0xdead_beef);
    }
}
