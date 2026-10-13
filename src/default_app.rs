//! Registering Seesvee as the default app for CSV files via LaunchServices.

use std::path::PathBuf;

pub const BUNDLE_ID: &str = "dev.seesvee.app";
const CSV_TYPE: &str = "public.comma-separated-values-text";

/// The enclosing `.app` bundle, when running from one.
pub fn app_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.ends_with("MacOS")
        && contents.ends_with("Contents")
        && bundle.extension().is_some_and(|e| e == "app"))
    .then(|| bundle.to_path_buf())
}

fn marker() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/Seesvee/default-app-registered"))
}

/// Claims `.csv` files the first time the bundled app runs. Later runs leave
/// the choice alone, so picking another default in Finder sticks.
pub fn register_on_first_launch() {
    let (Some(bundle), Some(marker)) = (app_bundle(), marker()) else {
        return;
    };
    if marker.exists() {
        return;
    }
    if make_default(&bundle).is_ok() {
        if let Some(dir) = marker.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        std::fs::write(&marker, BUNDLE_ID).ok();
    }
}

#[cfg(target_os = "macos")]
pub fn is_default() -> bool {
    mac::default_handler(CSV_TYPE).is_some_and(|id| id.eq_ignore_ascii_case(BUNDLE_ID))
}

#[cfg(target_os = "macos")]
pub fn make_default(bundle: &std::path::Path) -> Result<(), i32> {
    mac::register(bundle);
    mac::set_default_handler(CSV_TYPE, BUNDLE_ID)
}

#[cfg(not(target_os = "macos"))]
pub fn is_default() -> bool {
    false
}

#[cfg(not(target_os = "macos"))]
pub fn make_default(_: &std::path::Path) -> Result<(), i32> {
    Err(-1)
}

#[cfg(target_os = "macos")]
mod mac {
    use std::path::Path;

    use core_foundation::base::TCFType;
    use core_foundation::string::{CFString, CFStringRef};
    use core_foundation::url::{CFURL, CFURLRef};

    const ROLES_ALL: u32 = 0xFFFF_FFFF;

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn LSCopyDefaultRoleHandlerForContentType(
            content_type: CFStringRef,
            role: u32,
        ) -> CFStringRef;
        fn LSSetDefaultRoleHandlerForContentType(
            content_type: CFStringRef,
            role: u32,
            bundle_id: CFStringRef,
        ) -> i32;
        fn LSRegisterURL(url: CFURLRef, update: u8) -> i32;
    }

    pub fn default_handler(content_type: &str) -> Option<String> {
        let content_type = CFString::new(content_type);
        let handler = unsafe {
            LSCopyDefaultRoleHandlerForContentType(content_type.as_concrete_TypeRef(), ROLES_ALL)
        };
        (!handler.is_null())
            .then(|| unsafe { CFString::wrap_under_create_rule(handler) }.to_string())
    }

    pub fn register(bundle: &Path) {
        if let Some(url) = CFURL::from_path(bundle, true) {
            unsafe { LSRegisterURL(url.as_concrete_TypeRef(), 1) };
        }
    }

    pub fn set_default_handler(content_type: &str, bundle_id: &str) -> Result<(), i32> {
        let content_type = CFString::new(content_type);
        let bundle_id = CFString::new(bundle_id);
        let status = unsafe {
            LSSetDefaultRoleHandlerForContentType(
                content_type.as_concrete_TypeRef(),
                ROLES_ALL,
                bundle_id.as_concrete_TypeRef(),
            )
        };
        if status == 0 { Ok(()) } else { Err(status) }
    }
}
