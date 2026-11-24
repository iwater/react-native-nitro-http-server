// Global logging level atomic is defined in global.rs

// Log constants
pub const LOG_OFF: u8 = 0;
pub const LOG_ERROR: u8 = 1;
pub const LOG_WARN: u8 = 2;
pub const LOG_INFO: u8 = 3;
pub const LOG_DEBUG: u8 = 4;

#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => ({
        if $crate::global::VERBOSE.load(std::sync::atomic::Ordering::Relaxed) >= $crate::logger::LOG_ERROR {
            eprintln!("[ERROR] {}", format!($($arg)*));
        }
    })
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => ({
        if $crate::global::VERBOSE.load(std::sync::atomic::Ordering::Relaxed) >= $crate::logger::LOG_WARN {
            println!("[WARN] {}", format!($($arg)*));
        }
    })
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => ({
        if $crate::global::VERBOSE.load(std::sync::atomic::Ordering::Relaxed) >= $crate::logger::LOG_INFO {
            println!("[INFO] {}", format!($($arg)*));
        }
    })
}

#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => ({
        if $crate::global::VERBOSE.load(std::sync::atomic::Ordering::Relaxed) >= $crate::logger::LOG_DEBUG {
            println!("[DEBUG] {}", format!($($arg)*));
        }
    })
}
